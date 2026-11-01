use std::sync::Arc;

use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Request, Response, Version};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use tracing::debug;

use super::forwarded::{
    Client, FORWARDED_FOR, FORWARDED_HOST, FORWARDED_PORT, FORWARDED_PREFIX, FORWARDED_PROTO,
    REAL_IP, REQUEST_ID, strip_incoming,
};
use super::idle::Activity;
use super::pages::Page;
use super::site::{Hop, SiteRun};
use super::{Body, BoxError};
use crate::{Ask, BoxIo, Io, Refused, Upstream, Visitor};
use pool::{Back, H1, Key, Pool};
use send::Asking;

pub(crate) use pool::Ups;

pub mod pool;
pub(crate) mod send;
pub mod target_tls;
pub mod to_h2;
mod upgrade;

pub(crate) struct Forward<'a> {
    pub pool: &'a Arc<Pool>,
    pub epoch: u64,
    pub site: &'a SiteRun,
    pub visitor: Visitor,
    pub client: Client,
    pub prefix: Option<&'a str>,
    pub request_id: Option<&'a str>,
    pub activity: &'a Arc<Activity>,
}

impl Forward<'_> {
    fn page(&self, page: Page) -> Response<Body> {
        page.response_from(&self.site.pages)
    }

    fn asking<'a, U: Upstream>(
        &'a self,
        up: &'a U,
        deadline: tokio::time::Instant,
    ) -> Result<Asking<'a, U>, Refused> {
        let (way, lease) = up.lease(&self.site.service, &self.visitor, Ask::Site)?;
        let key = Key::new(&self.site.name, &way, self.site.app_key);
        Ok(Asking {
            up,
            pool: self.pool,
            service: &self.site.name,
            from: &self.visitor,
            hop: &self.site.hop,
            way,
            lease,
            key,
            deadline,
            epoch: self.epoch,
            ask: Ask::Site,
        })
    }
}

pub(crate) enum Failed {
    Refused(Refused),
    Broken,
}

pub(super) enum HopError {
    NoH2,
    Tls(std::io::Error),
    Broken,
}

impl std::fmt::Display for HopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HopError::NoH2 => f.write_str("the app did not agree to HTTP/2 over TLS (ALPN h2)"),
            HopError::Tls(e) => write!(f, "TLS to the app failed: {e}"),
            HopError::Broken => f.write_str("target_tls cannot be used"),
        }
    }
}

impl From<HopError> for Failed {
    fn from(e: HopError) -> Failed {
        debug!("web: {e}");
        Failed::Broken
    }
}

pub(super) async fn hop(io: BoxIo, how: &Hop, http2: bool) -> Result<BoxIo, HopError> {
    match how {
        Hop::Plain => Ok(io),
        Hop::Tls(connector, name) => match connector.connect(name.clone(), io).await {
            Ok(tls) if http2 && tls.get_ref().1.alpn_protocol() != Some(crate::terminate::H2) => {
                Err(HopError::NoH2)
            }
            Ok(tls) => Ok(Box::new(tls)),
            Err(e) => Err(HopError::Tls(e)),
        },
        Hop::Broken => Err(HopError::Broken),
    }
}

pub(crate) async fn handshake(io: BoxIo, how: &Hop) -> Result<H1, Failed> {
    let ending = io.ending();
    let (io, read) = pool::Counted::new(hop(io, how, false).await?);
    let (tx, conn) = super::h2::app_http1()
        .handshake(TokioIo::new(io))
        .await
        .map_err(|_| Failed::Broken)?;
    let task = tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            debug!("web: the connection to the app ended: {e}");
        }
    });
    Ok(H1 {
        tx,
        ending,
        task: task.abort_handle(),
        read,
    })
}

pub(crate) fn refusal_page(why: Refused) -> Page {
    match why {
        Refused::NotAllowed => Page::Forbidden,
        Refused::Full => Page::Busy,
        Refused::NoClient => Page::Unavailable,
        Refused::Gone => Page::NotFound,
        Refused::Down => Page::Unavailable,
        Refused::Denied | Refused::Unreachable => Page::BadGateway,
    }
}

pub(crate) fn connection_tokens(h: &HeaderMap) -> Vec<HeaderName> {
    h.get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|t| HeaderName::from_bytes(t.trim().as_bytes()).ok())
        .collect()
}

pub(crate) fn protected(name: &HeaderName) -> bool {
    *name == header::HOST || super::forwarded::managed(name)
}

pub fn strip_hop_by_hop(h: &mut HeaderMap) {
    for name in connection_tokens(h) {
        if !protected(&name) {
            h.remove(name);
        }
    }
    for name in burrow_syntax::web::headers::HOP_BY_HOP {
        h.remove(name);
    }
}

pub fn upgrade_of(h: &HeaderMap) -> Option<HeaderValue> {
    if !connection_tokens(h).contains(&header::UPGRADE) {
        return None;
    }
    super::headers::has_token(h, header::UPGRADE, "websocket")
        .then(|| HeaderValue::from_static("websocket"))
}

pub(crate) fn set(h: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        h.insert(HeaderName::from_static(name), v);
    }
}

pub fn join_cookies(h: &mut HeaderMap) {
    let crumbs: Vec<&[u8]> = h
        .get_all(header::COOKIE)
        .iter()
        .map(HeaderValue::as_bytes)
        .collect();
    if crumbs.len() < 2 {
        return;
    }
    if let Ok(v) = HeaderValue::from_bytes(&crumbs.join(&b"; "[..])) {
        h.insert(header::COOKIE, v);
    }
}

pub fn rewrite_request<B>(req: &mut Request<B>, host: &str, client: &Client) {
    if req.version() == Version::HTTP_2 {
        join_cookies(req.headers_mut());
        *req.version_mut() = Version::HTTP_11;
    }
    let upgrade = upgrade_of(req.headers());
    let h = req.headers_mut();
    strip_hop_by_hop(h);
    strip_incoming(h, client.trusted_peer);
    set(h, FORWARDED_FOR, &client.forwarded_for);
    set(h, REAL_IP, &client.ip.to_string());
    set(
        h,
        FORWARDED_PROTO,
        if client.https { "https" } else { "http" },
    );
    if !h.contains_key(FORWARDED_HOST) {
        set(h, FORWARDED_HOST, host);
    }
    set(h, FORWARDED_PORT, &client.port.to_string());
    if let Some(u) = upgrade {
        h.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        h.insert(header::UPGRADE, u);
    }
}

fn prepare(req: &mut Request<Body>, f: &Forward<'_>) {
    rewrite_request(req, &f.visitor.host, &f.client);
    extras(req, f);
}

pub(super) fn extras(req: &mut Request<Body>, f: &Forward<'_>) {
    let h = req.headers_mut();
    if let Some(p) = f.prefix {
        set(h, FORWARDED_PREFIX, p);
    }
    if let Some(id) = f.request_id {
        set(h, REQUEST_ID, id);
    }
    super::headers::apply(&f.site.options.request_headers, h);
}

pub(crate) async fn forward<U: Upstream>(
    up: &U,
    ups: &Ups,
    mut req: Request<Body>,
    f: Forward<'_>,
) -> Response<Body> {
    if f.site.options.target_http2 {
        return (to_h2::forward(up, ups, req, f)).await;
    }
    if upgrade_of(req.headers()).is_some() {
        return (upgrade::forward(up, ups, req, f)).await;
    }
    prepare(&mut req, &f);
    let deadline = tokio::time::Instant::now() + f.site.options.response_header_timeout;
    let Some(slot) = ups.slot(deadline).await else {
        return f.page(Page::Timeout);
    };
    let asking = match f.asking(up, deadline) {
        Ok(a) => a,
        Err(why) => return f.page(refusal_page(why)),
    };
    let (mut resp, mut home, ticket) = match asking.send_h1(Some(ups), req).await {
        Ok(r) => r,
        Err(miss) => return f.page(miss.page),
    };
    strip_hop_by_hop(resp.headers_mut());
    if private(resp.headers()) {
        ups.went_private();
        home.private(ups.id, f.visitor.addr.ip());
    }
    let lease = asking.lease;
    let lease = (lease, ticket);
    let activity = f.activity.clone();
    resp.map(move |b| {
        let b = b
            .map_err(move |e| {
                activity.fail();
                BoxError::from(e)
            })
            .boxed();
        Back::new(b, home, lease, Some(slot)).boxed()
    })
}

fn private(h: &HeaderMap) -> bool {
    h.get_all(header::WWW_AUTHENTICATE).iter().any(|v| {
        let scheme = v
            .as_bytes()
            .split(|b| *b == b' ')
            .next()
            .unwrap_or_default();
        scheme.eq_ignore_ascii_case(b"NTLM") || scheme.eq_ignore_ascii_case(b"Negotiate")
    })
}

#[cfg(test)]
mod tests;
