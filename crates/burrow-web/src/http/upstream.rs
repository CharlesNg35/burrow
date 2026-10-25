use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Request, Response, StatusCode, Uri};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::client::conn::http1::SendRequest;
use hyper_util::rt::TokioIo;
use tracing::debug;

use super::forwarded::{Client, strip_incoming};
use super::idle::Activity;
use super::pages::Page;
use super::{Body, BoxError, MAX_HEAD, MAX_HEADERS};
use crate::{Ending, Io, Refused, Upstream, Visitor};

pub mod target_tls;

const REUSE_WAIT: Duration = Duration::from_secs(1);

const HOP_BY_HOP: [HeaderName; 7] = [
    header::CONNECTION,
    HeaderName::from_static("keep-alive"),
    HeaderName::from_static("proxy-connection"),
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
];

struct Stream {
    service: String,
    tx: SendRequest<Body>,
    ending: Option<Ending>,
}

#[derive(Default)]
pub(crate) struct Upstreams {
    slot: Mutex<Option<Stream>>,
}

pub(crate) struct Forward<'a> {
    pub service: &'a str,
    pub visitor: Visitor,
    pub client: Client,
    pub response_header: Duration,
    pub activity: &'a Arc<Activity>,
}

enum Failed {
    Refused(Refused),
    Broken,
}

impl Upstreams {
    async fn reuse(&self, service: &str) -> Option<Stream> {
        let mut s = self.slot.lock().expect("upstream lock").take()?;
        if s.service != service {
            return None;
        }
        let ready = tokio::time::timeout(REUSE_WAIT, s.tx.ready()).await;
        matches!(ready, Ok(Ok(()))).then_some(s)
    }

    fn keep(&self, s: Stream) {
        *self.slot.lock().expect("upstream lock") = Some(s);
    }
}

async fn open<U: Upstream>(up: &U, f: &Forward<'_>) -> Result<Stream, Failed> {
    let io = up
        .open(f.service, &f.visitor)
        .await
        .map_err(Failed::Refused)?;
    let ending = io.ending();
    let (tx, conn) = hyper::client::conn::http1::Builder::new()
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
    Ok(Stream {
        service: f.service.to_string(),
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

fn connection_tokens(h: &HeaderMap) -> Vec<HeaderName> {
    h.get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|t| HeaderName::from_bytes(t.trim().as_bytes()).ok())
        .collect()
}

fn protected(name: &HeaderName) -> bool {
    *name == header::HOST || super::forwarded::managed(name)
}

pub fn strip_hop_by_hop(h: &mut HeaderMap) {
    for name in connection_tokens(h) {
        if !protected(&name) {
            h.remove(name);
        }
    }
    for name in &HOP_BY_HOP {
        h.remove(name);
    }
}

pub fn upgrade_of(h: &HeaderMap) -> Option<HeaderValue> {
    let asked = connection_tokens(h).contains(&header::UPGRADE);
    h.get(header::UPGRADE).filter(|_| asked).cloned()
}

fn set(h: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        h.insert(HeaderName::from_static(name), v);
    }
}

pub fn rewrite_request<B>(req: &mut Request<B>, host: &str, client: &Client) {
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
    set(h, "x-forwarded-for", &client.forwarded_for);
    set(h, "x-real-ip", &client.ip.to_string());
    set(
        h,
        "x-forwarded-proto",
        if client.https { "https" } else { "http" },
    );
    if !h.contains_key("x-forwarded-host") {
        set(h, "x-forwarded-host", host);
    }
    set(h, "x-forwarded-port", &client.port.to_string());
    if let Some(u) = upgrade {
        h.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        h.insert(header::UPGRADE, u);
    }
}

pub(crate) async fn forward<U: Upstream>(
    up: &U,
    ups: &Upstreams,
    mut req: Request<Incoming>,
    f: Forward<'_>,
) -> Response<Body> {
    let upgrading = upgrade_of(req.headers()).is_some();
    let downstream = upgrading.then(|| hyper::upgrade::on(&mut req));
    let host = f.visitor.host.clone();
    rewrite_request(&mut req, &host, &f.client);
    let req = req.map(|b| b.map_err(Into::into).boxed());
    let mut s = match ups.reuse(f.service).await {
        Some(s) => s,
        None => match open(up, &f).await {
            Ok(s) => s,
            Err(Failed::Refused(why)) => return refusal_page(why).response(),
            Err(Failed::Broken) => return Page::BadGateway.response(),
        },
    };
    let mut resp = match tokio::time::timeout(f.response_header, s.tx.send_request(req)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            debug!(peer = %f.visitor.addr, "web: service \"{}\" did not answer: {e}", f.service);
            let why = s.ending.as_ref().and_then(Ending::refused);
            return why.map_or(Page::BadGateway, refusal_page).response();
        }
        Err(_) => return Page::Timeout.response(),
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
        b.map_err(move |e| {
            activity.fail();
            BoxError::from(e)
        })
        .boxed()
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
