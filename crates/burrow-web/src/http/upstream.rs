use std::sync::Arc;

use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Request, Response, StatusCode, Uri, Version};
use http_body_util::BodyExt;
use hyper::client::conn::http1::SendRequest;
use hyper_util::rt::TokioIo;
use tracing::debug;

use super::forwarded::{
    Client, FORWARDED_FOR, FORWARDED_HOST, FORWARDED_PORT, FORWARDED_PREFIX, FORWARDED_PROTO,
    REAL_IP, REQUEST_ID, strip_incoming,
};
use super::idle::Activity;
use super::pages::Page;
use super::site::{Hop, SiteRun};
use super::{Body, BoxError, MAX_HEAD, MAX_HEADERS};
use crate::{BoxIo, Ending, Io, Refused, Upstream, Visitor};
use pool::{Slotted, Stream};

pub(crate) use pool::Upstreams;

pub mod pool;
pub mod target_tls;
pub mod to_h2;

pub(crate) struct Forward<'a> {
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
}

pub(crate) enum Failed {
    Refused(Refused),
    Broken,
}

pub(super) async fn hop(io: BoxIo, how: &Hop, http2: bool) -> Result<BoxIo, Failed> {
    match how {
        Hop::Plain => Ok(io),
        Hop::Tls(connector, name) => match connector.connect(name.clone(), io).await {
            Ok(tls) if http2 && tls.get_ref().1.alpn_protocol() != Some(b"h2") => {
                debug!("web: the app did not agree to HTTP/2 over TLS (ALPN h2)");
                Err(Failed::Broken)
            }
            Ok(tls) => Ok(Box::new(tls)),
            Err(e) => {
                debug!("web: TLS to the app failed: {e}");
                Err(Failed::Broken)
            }
        },
        Hop::Broken => Err(Failed::Broken),
    }
}

pub(crate) async fn handshake(
    io: BoxIo,
    how: &Hop,
) -> Result<(SendRequest<Body>, Option<Ending>), Failed> {
    let ending = io.ending();
    let io = hop(io, how, false).await?;
    let (tx, conn) = hyper::client::conn::http1::Builder::new()
        .max_buf_size(super::h2::APP_READ_BUF)
        .max_header_size(MAX_HEAD)
        .max_headers(MAX_HEADERS)
        .handshake(TokioIo::new(io))
        .await
        .map_err(|_| Failed::Broken)?;
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            debug!("web: the connection to the app ended: {e}");
        }
    });
    Ok((tx, ending))
}

async fn open<U: Upstream>(up: &U, f: &Forward<'_>) -> Result<Stream, Failed> {
    let io = up
        .open(&f.site.service, &f.visitor)
        .await
        .map_err(Failed::Refused)?;
    let (tx, ending) = handshake(io, &f.site.hop).await?;
    Ok(Stream {
        service: f.site.service.clone(),
        client: f.client.ip,
        tx,
        ending,
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
    let websocket = h
        .get_all(header::UPGRADE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|p| p.trim().eq_ignore_ascii_case("websocket"));
    websocket.then(|| HeaderValue::from_static("websocket"))
}

fn set(h: &mut HeaderMap, name: &'static str, value: &str) {
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
    if req.uri().authority().is_some() {
        if let Ok(v) = HeaderValue::from_str(host) {
            req.headers_mut().insert(header::HOST, v);
        }
        let origin = req.uri().path_and_query().map_or("/", |p| p.as_str());
        *req.uri_mut() = origin.parse().unwrap_or_else(|_| Uri::from_static("/"));
    }
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
    ups: &Upstreams,
    mut req: Request<Body>,
    f: Forward<'_>,
) -> Response<Body> {
    if f.site.options.target_http2 {
        return to_h2::forward(up, ups, req, f).await;
    }
    let upgrading = upgrade_of(req.headers()).is_some();
    let downstream = upgrading.then(|| hyper::upgrade::on(&mut req));
    prepare(&mut req, &f);
    let service = f.site.service.as_str();
    let deadline = tokio::time::Instant::now() + f.site.options.response_header_timeout;
    let Some(slot) = ups.slot(deadline).await else {
        return f.page(Page::Timeout);
    };
    let mut s = match ups.reuse(service, f.client.ip).await {
        Some(s) => s,
        None => match tokio::time::timeout_at(deadline, open(up, &f)).await {
            Ok(Ok(s)) => s,
            Ok(Err(Failed::Refused(why))) => return f.page(refusal_page(why)),
            Ok(Err(Failed::Broken)) => return f.page(Page::BadGateway),
            Err(_) => return f.page(Page::Timeout),
        },
    };
    let mut resp = match tokio::time::timeout_at(deadline, s.tx.send_request(req)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            debug!(peer = %f.visitor.addr, "web: service \"{service}\" did not answer: {e}");
            let why = s.ending.as_ref().and_then(Ending::refused);
            return f.page(why.map_or(Page::BadGateway, refusal_page));
        }
        Err(_) => return f.page(Page::Timeout),
    };
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        if let Some(down) = downstream {
            let up = hyper::upgrade::on(&mut resp);
            tokio::spawn(join(down, up, f.activity.clone()));
        }
        return resp.map(|b| b.map_err(Into::into).boxed());
    }
    strip_hop_by_hop(resp.headers_mut());
    if !upgrading {
        ups.keep(s);
    }
    let activity = f.activity.clone();
    resp.map(move |b| {
        let b = b
            .map_err(move |e| {
                activity.fail();
                BoxError::from(e)
            })
            .boxed();
        Slotted::new(b, slot).boxed()
    })
}

async fn join(
    down: hyper::upgrade::OnUpgrade,
    up: hyper::upgrade::OnUpgrade,
    activity: Arc<Activity>,
) {
    let (Ok(down), Ok(up)) = (down.await, up.await) else {
        return;
    };
    let (mut down, mut up) = (TokioIo::new(down), TokioIo::new(up));
    if let Err(e) = tokio::io::copy_bidirectional(&mut down, &mut up).await {
        debug!("web: an upgraded connection ended: {e}");
        activity.fail();
    }
}

#[cfg(test)]
mod tests;
