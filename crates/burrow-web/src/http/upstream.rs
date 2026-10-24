use std::sync::Mutex;
use std::time::Duration;

use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Request, Response, StatusCode, Uri};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::client::conn::http1::SendRequest;
use hyper_util::rt::TokioIo;
use tracing::debug;

use super::pages::Page;
use super::{Body, MAX_HEAD, MAX_HEADERS};
use crate::{Refused, Upstream, Visitor};

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

#[derive(Default)]
pub(crate) struct Upstreams {
    slot: Mutex<Option<(String, SendRequest<Body>)>>,
}

pub(crate) struct Forward<'a> {
    pub service: &'a str,
    pub visitor: Visitor,
    pub https: bool,
    pub response_header: Duration,
}

enum Failed {
    Refused(Refused),
    Broken,
}

impl Upstreams {
    async fn reuse(&self, service: &str) -> Option<SendRequest<Body>> {
        let (name, mut tx) = self.slot.lock().expect("upstream lock").take()?;
        if name != service {
            return None;
        }
        matches!(
            tokio::time::timeout(REUSE_WAIT, tx.ready()).await,
            Ok(Ok(()))
        )
        .then_some(tx)
    }

    fn keep(&self, service: &str, tx: SendRequest<Body>) {
        *self.slot.lock().expect("upstream lock") = Some((service.to_string(), tx));
    }
}

async fn open<U: Upstream>(up: &U, f: &Forward<'_>) -> Result<SendRequest<Body>, Failed> {
    let io = up
        .open(f.service, &f.visitor)
        .await
        .map_err(Failed::Refused)?;
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
    Ok(tx)
}

pub(crate) fn refusal_page(why: Refused) -> Page {
    match why {
        Refused::NotAllowed => Page::Forbidden,
        Refused::Full => Page::Busy,
        Refused::NoClient => Page::Unavailable,
        Refused::Gone => Page::NotFound,
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

pub fn strip_hop_by_hop(h: &mut HeaderMap) {
    for name in connection_tokens(h) {
        h.remove(name);
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

pub fn rewrite_request<B>(req: &mut Request<B>, host: &str, from: &Visitor, https: bool) {
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
    let client = from.addr.ip().to_canonical().to_string();
    set(h, "x-forwarded-for", &client);
    set(h, "x-real-ip", &client);
    set(h, "x-forwarded-proto", if https { "https" } else { "http" });
    set(h, "x-forwarded-host", host);
    set(h, "x-forwarded-port", &from.local.port().to_string());
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
    rewrite_request(&mut req, &host, &f.visitor, f.https);
    let req = req.map(|b| b.map_err(Into::into).boxed());
    let mut tx = match ups.reuse(f.service).await {
        Some(tx) => tx,
        None => match open(up, &f).await {
            Ok(tx) => tx,
            Err(Failed::Refused(why)) => return refusal_page(why).response(),
            Err(Failed::Broken) => return Page::BadGateway.response(),
        },
    };
    let mut resp = match tokio::time::timeout(f.response_header, tx.send_request(req)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            debug!(peer = %f.visitor.addr, "web: service \"{}\" did not answer: {e}", f.service);
            return Page::BadGateway.response();
        }
        Err(_) => return Page::Timeout.response(),
    };
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        if let Some(down) = downstream {
            let up = hyper::upgrade::on(&mut resp);
            tokio::spawn(join(down, up));
        }
        return resp.map(|b| b.map_err(Into::into).boxed());
    }
    strip_hop_by_hop(resp.headers_mut());
    if !upgrading {
        ups.keep(f.service, tx);
    }
    resp.map(|b| b.map_err(Into::into).boxed())
}

async fn join(down: hyper::upgrade::OnUpgrade, up: hyper::upgrade::OnUpgrade) {
    let (Ok(down), Ok(up)) = (down.await, up.await) else {
        return;
    };
    let (mut down, mut up) = (TokioIo::new(down), TokioIo::new(up));
    if let Err(e) = tokio::io::copy_bidirectional(&mut down, &mut up).await {
        debug!("web: an upgraded connection ended: {e}");
    }
}

#[cfg(test)]
mod tests;
