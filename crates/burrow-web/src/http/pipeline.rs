use std::sync::Arc;

use burrow_syntax::url_path::normalize_path;
use bytes::Bytes;
use http::{HeaderValue, Method, Request, Response, StatusCode, Uri, header};
use http_body_util::BodyExt;
use hyper::body::Incoming;

use super::forwarded;
use super::host::request_host;
use super::pages::Page;
use super::redirect;
use super::stages::{Asked, to_service};
use super::{Body, Conn, MAX_HEADERS, full};
use crate::Upstream;
use crate::listen::Shared;
use crate::route::{Action, Kind, Table};
use crate::tls::CertState;
use crate::tunnel::{self, Answer, Handoff};

const CHALLENGE: &str = "/.well-known/acme-challenge/";

pub(crate) async fn handle<U: Upstream>(
    shared: &Arc<Shared<U>>,
    conn: &Conn,
    mut req: Request<Incoming>,
) -> Response<Body> {
    if req.headers().len() > MAX_HEADERS {
        return Page::HeadersTooLarge.response();
    }
    let Some(host) = request_host(req.uri(), req.headers(), req.version()) else {
        return Page::BadRequest.response();
    };
    if req.method() == Method::CONNECT {
        return Page::BadRequest.response();
    }
    match &conn.name {
        Some(name) if *name != host => return Page::Misdirected.response(),
        Some(_) => {}
        None => {
            if let Some(answer) = challenge(shared, &req) {
                return answer;
            }
        }
    }
    if !normalize_target(&mut req, &host) {
        return Page::BadRequest.response();
    }
    let table = shared.table();
    if conn.name.is_none() {
        if let Some(prefix) = table.plain_tunnel(&host, req.uri().path()) {
            return by_path(shared, conn, req, prefix);
        }
        if let Some(answer) = on_plain(shared, conn, &table, &host, &req) {
            return answer;
        }
    }
    let Some(route) = table.route(&host, req.uri().path()) else {
        return Page::NotFound.response();
    };
    match &route.action {
        Action::Tunnel if conn.name.is_some() => by_path(shared, conn, req, &route.prefix),
        Action::Tunnel => Page::NotFound.response(),
        Action::Redirect { to, permanent } => {
            let l = redirect::location(&route.prefix, to, req.uri().path(), req.uri().query());
            redirect::response(&l, *permanent)
        }
        Action::Service(service) => {
            let asked = Asked {
                host,
                route: route.clone(),
                service: service.clone(),
            };
            to_service(shared, conn, &table, asked, req).await
        }
    }
}

fn normalize_target<B>(req: &mut Request<B>, host: &str) -> bool {
    let Ok(path) = normalize_path(req.uri().path()) else {
        return false;
    };
    let target = match req.uri().query() {
        Some(q) => format!("{path}?{q}"),
        None => path,
    };
    let Ok(uri) = target.parse::<Uri>() else {
        return false;
    };
    if req.uri().authority().is_some() {
        let Ok(v) = HeaderValue::from_str(host) else {
            return false;
        };
        req.headers_mut().insert(header::HOST, v);
    }
    *req.uri_mut() = uri;
    true
}

fn challenge_token(path: &str) -> Option<&str> {
    let token = path.strip_prefix(CHALLENGE)?;
    let good = !token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    good.then_some(token)
}

fn only_tunnel(kind: &Kind) -> bool {
    matches!(kind, Kind::Http { routes, .. } if routes.iter().all(|r| r.action == Action::Tunnel))
}

fn challenge<U: Upstream>(shared: &Shared<U>, req: &Request<Incoming>) -> Option<Response<Body>> {
    let answer = challenge_token(req.uri().path()).and_then(|t| shared.certs.http_challenge(t))?;
    let mut r = Response::new(full(Bytes::from(answer)));
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    Some(r)
}

fn on_plain<U: Upstream>(
    shared: &Shared<U>,
    conn: &Conn,
    table: &Table,
    host: &str,
    req: &Request<Incoming>,
) -> Option<Response<Body>> {
    let proxied_https = || {
        let trusted = &table.forwarding.trusted_proxies;
        forwarded::client(conn.peer, req.headers(), trusted, false).https
    };
    match table.kind(host) {
        Some(Kind::Plain {
            redirect: false, ..
        }) => None,
        Some(Kind::Plain { redirect: true, .. }) if proxied_https() => None,
        Some(Kind::Plain { redirect: true, .. }) => Some(to_https(host, None, req)),
        Some(
            k @ Kind::Http {
                redirect: false, ..
            },
        ) if !only_tunnel(k) => None,
        Some(k @ Kind::Http { redirect: true, .. }) if !only_tunnel(k) && proxied_https() => None,
        Some(k @ Kind::Http { redirect: true, .. }) if !only_tunnel(k) => {
            Some(match shared.certs.state(host) {
                CertState::Ready => to_https(host, shared.https_port, req),
                CertState::Pending | CertState::Failing => Page::Pending.response(),
            })
        }
        _ => Some(Page::NotFound.response()),
    }
}

pub fn https_location(host: &str, port: Option<u16>, path_and_query: &str) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    match port {
        Some(p) if p != 443 => format!("https://{host}:{p}{path_and_query}"),
        _ => format!("https://{host}{path_and_query}"),
    }
}

fn to_https(host: &str, port: Option<u16>, req: &Request<Incoming>) -> Response<Body> {
    let pq = req.uri().path_and_query().map_or("/", |p| p.as_str());
    let mut r = Response::new(full(Bytes::new()));
    *r.status_mut() = StatusCode::PERMANENT_REDIRECT;
    if let Ok(v) = HeaderValue::from_str(&https_location(host, port, pq)) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

fn by_path<U: Upstream>(
    shared: &Arc<Shared<U>>,
    conn: &Conn,
    mut req: Request<Incoming>,
    prefix: &str,
) -> Response<Body> {
    let answer = tunnel::answer(&req, prefix);
    if matches!(answer, Answer::Upgrade(_)) {
        let Some(permit) = shared.up.tunnel_permit(conn.peer) else {
            return Page::Busy.response();
        };
        let on = hyper::upgrade::on(&mut req);
        let handoff = Handoff {
            path: prefix.to_string(),
            peer: conn.peer,
            local: conn.local,
            deadline: tokio::time::Instant::now() + shared.timeouts().handshake,
            permit,
        };
        let up = shared.up.clone();
        tokio::spawn(async move { tunnel::take_over(up.as_ref(), on, handoff).await });
    }
    tunnel::reply(&answer).map(|b| b.map_err(|never| match never {}).boxed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_non_empty_base64url_token_is_a_challenge() {
        assert_eq!(
            challenge_token("/.well-known/acme-challenge/abc_DEF-9"),
            Some("abc_DEF-9")
        );
        assert_eq!(challenge_token("/.well-known/acme-challenge/"), None);
        assert_eq!(challenge_token("/.well-known/acme-challenge/a/b"), None);
        assert_eq!(challenge_token("/.well-known/acme-challenge/a.b"), None);
        assert_eq!(challenge_token("/other"), None);
    }

    #[test]
    fn the_https_address_keeps_the_path_and_names_a_port_other_than_443() {
        assert_eq!(
            https_location("a.example.com", Some(443), "/x?y=1"),
            "https://a.example.com/x?y=1"
        );
        assert_eq!(
            https_location("a.example.com", None, "/"),
            "https://a.example.com/"
        );
        assert_eq!(
            https_location("a.example.com", Some(8443), "/"),
            "https://a.example.com:8443/"
        );
        assert_eq!(
            https_location("2001:db8::1", Some(443), "/"),
            "https://[2001:db8::1]/"
        );
    }
}
