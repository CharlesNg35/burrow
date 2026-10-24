use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Version, header};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::{TokioIo, TokioTimer};
use tracing::debug;

use crate::io::{BoxIo, Prefixed};
use crate::listen::Started;
use crate::route::{Action, Kind, Table};
use crate::{TunnelConn, Upstream};

pub const MAX_HEAD: usize = 8 * 1024;
pub const MAX_HEADERS: usize = 32;
pub const WEBSOCKET_VERSION: &str = "13";
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub fn accept_key(key: &str) -> String {
    let mut input = key.as_bytes().to_vec();
    input.extend_from_slice(WEBSOCKET_GUID.as_bytes());
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &input);
    base64::engine::general_purpose::STANDARD.encode(digest.as_ref())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Upgrade(String),
    NotFound,
    UpgradeRequired,
    BadRequest,
}

fn has_token(headers: &HeaderMap, name: header::HeaderName, token: &str) -> bool {
    headers.get_all(name).iter().any(|v| {
        v.to_str()
            .is_ok_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    })
}

fn good_key(key: &str) -> bool {
    base64::engine::general_purpose::STANDARD
        .decode(key)
        .is_ok_and(|k| k.len() == 16)
}

pub fn answer<B>(req: &Request<B>, tunnel_path: &str) -> Answer {
    let path = req.uri().path();
    if !crate::route::on_prefix(tunnel_path, path) {
        return Answer::NotFound;
    }
    let h = req.headers();
    let upgrade = req.method() == Method::GET
        && req.version() == Version::HTTP_11
        && has_token(h, header::UPGRADE, "websocket")
        && has_token(h, header::CONNECTION, "upgrade");
    if path != tunnel_path || !upgrade {
        return Answer::UpgradeRequired;
    }
    let version = h
        .get(header::SEC_WEBSOCKET_VERSION)
        .and_then(|v| v.to_str().ok());
    if version.map(str::trim) != Some(WEBSOCKET_VERSION) {
        return Answer::UpgradeRequired;
    }
    match h
        .get(header::SEC_WEBSOCKET_KEY)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
    {
        Some(key) if good_key(key) => Answer::Upgrade(accept_key(key)),
        _ => Answer::BadRequest,
    }
}

fn reply(answer: &Answer) -> Response<Full<Bytes>> {
    let (status, text) = match answer {
        Answer::Upgrade(_) => (StatusCode::SWITCHING_PROTOCOLS, ""),
        Answer::NotFound => (StatusCode::NOT_FOUND, "Not found.\n"),
        Answer::UpgradeRequired => (
            StatusCode::UPGRADE_REQUIRED,
            "This address is for burrow clients.\n",
        ),
        Answer::BadRequest => (StatusCode::BAD_REQUEST, "Bad request.\n"),
    };
    let mut r = Response::new(Full::new(Bytes::from_static(text.as_bytes())));
    *r.status_mut() = status;
    let h = r.headers_mut();
    match answer {
        Answer::Upgrade(accept) => {
            h.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
            h.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
            if let Ok(v) = HeaderValue::from_str(accept) {
                h.insert(header::SEC_WEBSOCKET_ACCEPT, v);
            }
        }
        Answer::UpgradeRequired => {
            h.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
            h.insert(
                header::SEC_WEBSOCKET_VERSION,
                HeaderValue::from_static(WEBSOCKET_VERSION),
            );
            h.insert(header::CONNECTION, HeaderValue::from_static("close"));
        }
        _ => {
            h.insert(header::CONNECTION, HeaderValue::from_static("close"));
        }
    }
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

fn tunnel_path(table: &Table, host: &str) -> Option<String> {
    match table.kind(host)? {
        Kind::Http(routes) => routes
            .iter()
            .find(|r| r.action == Action::Tunnel)
            .map(|r| r.prefix.clone()),
        _ => None,
    }
}

pub(crate) async fn serve_by_path<U: Upstream>(
    up: Arc<U>,
    table: &Table,
    host: &str,
    started: Started<U::Permit>,
) {
    let Some(path) = tunnel_path(table, host) else {
        return;
    };
    let Started {
        io,
        peer,
        local,
        deadline,
        permit,
    } = started;
    let upgrade: Arc<Mutex<Option<OnUpgrade>>> = Arc::default();
    let service = {
        let (upgrade, path) = (upgrade.clone(), path.clone());
        hyper::service::service_fn(move |mut req: Request<Incoming>| {
            let answer = answer(&req, &path);
            if matches!(answer, Answer::Upgrade(_)) {
                *upgrade.lock().expect("upgrade lock") = Some(hyper::upgrade::on(&mut req));
            }
            std::future::ready(Ok::<_, Infallible>(reply(&answer)))
        })
    };
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let conn = hyper::server::conn::http1::Builder::new()
        .max_header_size(MAX_HEAD)
        .max_headers(MAX_HEADERS)
        .keep_alive(false)
        .timer(TokioTimer::new())
        .header_read_timeout(remaining)
        .serve_connection(TokioIo::new(io), service)
        .with_upgrades();
    if let Err(e) = tokio::time::timeout_at(deadline, conn)
        .await
        .unwrap_or(Ok(()))
    {
        debug!(%peer, "web: the tunnel request failed: {e}");
    }
    let Some(on) = upgrade.lock().expect("upgrade lock").take() else {
        return;
    };
    let Ok(Ok(upgraded)) = tokio::time::timeout_at(deadline, on).await else {
        return;
    };
    let Ok(parts) = upgraded.downcast::<TokioIo<BoxIo>>() else {
        return;
    };
    let io = Prefixed::new(parts.read_buf.to_vec(), parts.io.into_inner());
    up.tunnel(TunnelConn {
        io: Box::new(io),
        upgraded: true,
        path,
        peer,
        local,
        deadline,
        permit,
    })
    .await;
}

#[cfg(test)]
mod tests;
