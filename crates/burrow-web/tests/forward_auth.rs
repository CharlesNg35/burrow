mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow_web::{ForwardAuth, Refused, Site};
use bytes::Bytes;
use common::*;
use http::{HeaderName, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use tokio::net::TcpListener;

type Seen = Arc<Mutex<Vec<String>>>;

fn head(req: &Request<Incoming>) -> String {
    let mut text = format!("{} {}\n", req.method(), req.uri());
    for (k, v) in req.headers() {
        text.push_str(&format!("{k}: {}\n", v.to_str().unwrap_or("?")));
    }
    text
}

async fn decide(req: Request<Incoming>, seen: Seen) -> Response<Full<Bytes>> {
    let want = req
        .headers()
        .get("x-want")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut text = head(&req);
    let body = req
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes().len())
        .unwrap_or(0);
    text.push_str(&format!("body-len: {body}\n"));
    seen.lock().unwrap().push(text);
    let mut r = Response::new(Full::new(Bytes::from_static(b"from auth")));
    if let Ok(code) = want.parse::<u16>() {
        *r.status_mut() = StatusCode::from_u16(code).unwrap();
    }
    let h = r.headers_mut();
    match want.as_str() {
        "alice" => {
            h.insert("remote-user", "alice".parse().unwrap());
            h.append("remote-groups", "dev".parse().unwrap());
            h.append("remote-groups", "ops".parse().unwrap());
            h.insert("set-cookie", "granted=1".parse().unwrap());
        }
        "none" => {}
        "slow" => tokio::time::sleep(Duration::from_secs(3)).await,
        _ => {
            h.insert(
                "location",
                "https://auth.example.com/?rd=x".parse().unwrap(),
            );
            h.insert(
                "www-authenticate",
                "Basic realm=\"authelia\"".parse().unwrap(),
            );
            h.insert(
                "proxy-authenticate",
                "Basic realm=\"authelia\"".parse().unwrap(),
            );
            h.insert("set-cookie", "authelia_session=new".parse().unwrap());
            h.insert("keep-alive", "timeout=5".parse().unwrap());
        }
    }
    r
}

async fn auth_server() -> (SocketAddr, Seen) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Seen::default();
    let kept = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            let seen = kept.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(move |req| {
                    let seen = seen.clone();
                    async move { Ok::<_, std::convert::Infallible>(decide(req, seen).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    (addr, seen)
}

fn copy() -> Vec<HeaderName> {
    [
        "remote-user",
        "remote-groups",
        "remote-email",
        "remote-name",
    ]
    .iter()
    .map(|n| HeaderName::from_static(n))
    .collect()
}

fn wiki(timeout: Duration) -> Site {
    let mut s = https_site("wiki", &["wiki.example.com"]);
    s.options.forward_auth = Some(ForwardAuth {
        service: "authelia".into(),
        path: "/api/authz/forward-auth".into(),
        copy_headers: copy(),
    });
    s.options.response_header_timeout = timeout;
    s
}

struct World {
    proxy: burrow_web::Proxy<Fake>,
    up: Arc<Fake>,
    seen: Seen,
    app: HttpBackend,
}

async fn world(timeout: Duration) -> World {
    let app = http_backend().await;
    let (auth, seen) = auth_server().await;
    let (up, _) = Fake::new(4);
    up.serve("wiki", app.addr);
    up.serve("authelia", auth);
    let proxy = start(
        |s| s.sites = vec![wiki(timeout), https_site("authelia", &["auth.example.com"])],
        up.clone(),
    )
    .await;
    World {
        proxy,
        up,
        seen,
        app,
    }
}

impl World {
    async fn send(&self, req: &str) -> String {
        over_tls(self.proxy.https_addr().unwrap(), "wiki.example.com", req).await
    }

    async fn get(&self, want: &str, extra: &str) -> String {
        self.send(&format!(
            "GET /admin/x?y=1 HTTP/1.1\r\nHost: wiki.example.com\r\nX-Want: {want}\r\n{extra}Connection: close\r\n\r\n"
        ))
        .await
    }

    fn asked(&self) -> String {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }

    fn app_calls(&self) -> usize {
        self.app
            .connections
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn app_part(r: &str) -> String {
    r.split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .to_ascii_lowercase()
}

#[tokio::test]
async fn a_grant_passes_the_auth_services_user_and_never_the_visitors() {
    let w = world(Duration::from_secs(5)).await;
    let r = w
        .get("alice", "Remote-User: root\r\nRemote_User: root\r\nREMOTE-USER: root\r\nremote_groups: admins\r\n")
        .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let app = app_part(&r);
    assert!(app.contains("remote-user: alice\n"), "{r}");
    assert!(!app.contains("root"), "{r}");
    assert!(!app.contains("admins"), "{r}");
    assert_eq!(app.matches("remote-groups: ").count(), 2, "{r}");
    assert!(!r.to_ascii_lowercase().contains("set-cookie"), "{r}");
    assert!(
        !w.asked().to_ascii_lowercase().contains("root"),
        "{}",
        w.asked()
    );
}

#[tokio::test]
async fn a_grant_without_a_user_still_drops_the_visitors_copy() {
    let w = world(Duration::from_secs(5)).await;
    let r = w
        .get(
            "none",
            "Remote-User: root\r\nRemote_User: root\r\nremote-email: x@evil\r\n",
        )
        .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let app = app_part(&r);
    assert!(!app.contains("remote-user"), "{r}");
    assert!(!app.contains("remote_user"), "{r}");
    assert!(!app.contains("remote-email"), "{r}");
}

#[tokio::test]
async fn a_refusal_goes_back_to_the_visitor_and_the_app_is_not_called() {
    let w = world(Duration::from_secs(5)).await;
    for (code, header) in [
        ("302", "location: https://auth.example.com/?rd=x"),
        ("401", "www-authenticate: basic realm=\"authelia\""),
        ("403", "set-cookie: authelia_session=new"),
        ("407", "proxy-authenticate: basic realm=\"authelia\""),
    ] {
        let r = w.get(code, "").await;
        assert!(r.starts_with(&format!("HTTP/1.1 {code}")), "{r}");
        let lower = r.to_ascii_lowercase();
        assert!(lower.contains(header), "{code}: {r}");
        assert!(!lower.contains("keep-alive: timeout"), "{code}: {r}");
        assert!(r.ends_with("from auth"), "{code}: {r}");
    }
    assert_eq!(w.app_calls(), 0);
}

#[tokio::test]
async fn the_auth_service_gets_a_bodiless_get_with_burrows_own_forwarded_values() {
    let w = world(Duration::from_secs(5)).await;
    let r = w
        .send(
            "POST /admin/x?y=1 HTTP/1.1\r\nHost: wiki.example.com\r\nX-Want: alice\r\nTransfer-Encoding: chunked\r\nX-Forwarded-For: 169.254.1.2\r\nX-Forwarded-Uri: /public\r\nX-Original-URL: https://wiki.example.com/public\r\nX-Original-URI: /public\r\nX_Original_URI: /public\r\nX-Original-Method: GET\r\nX-Forwarded-Port: 1\r\nForwarded: for=1.2.3.4\r\nX-Forwarded-Prefix: /p\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        )
        .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(
        app_part(&r).contains("body-len: 5\n"),
        "the app gets the body: {r}"
    );
    let asked = w.asked();
    assert!(
        asked.starts_with("GET /api/authz/forward-auth\n"),
        "{asked}"
    );
    assert!(asked.contains("body-len: 0\n"), "{asked}");
    for gone in [
        "transfer-encoding",
        "content-length",
        "x-forwarded-prefix",
        "x-forwarded-port",
        "forwarded:",
        "x_original_uri",
        "169.254.1.2",
        "/public",
        "1.2.3.4",
    ] {
        assert!(!asked.contains(gone), "{gone}: {asked}");
    }
    for want in [
        "x-forwarded-for: 127.0.0.1\n",
        "x-forwarded-method: POST\n",
        "x-forwarded-proto: https\n",
        "x-forwarded-host: wiki.example.com\n",
        "x-forwarded-uri: /admin/x?y=1\n",
        "x-original-url: https://wiki.example.com/admin/x?y=1\n",
        "x-original-uri: /admin/x?y=1\n",
        "x-original-method: POST\n",
        "\nhost: wiki.example.com\n",
    ] {
        assert_eq!(asked.matches(want).count(), 1, "{want}: {asked}");
    }
}

#[tokio::test]
async fn connection_tokens_cannot_remove_the_users_burrow_adds() {
    let w = world(Duration::from_secs(5)).await;
    let r = w
        .get("alice", "Connection: Remote-User, X-Forwarded-For\r\n")
        .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let app = app_part(&r);
    assert!(app.contains("remote-user: alice\n"), "{r}");
    assert!(app.contains("x-forwarded-for: 127.0.0.1\n"), "{r}");
}

#[tokio::test]
async fn a_slow_or_missing_auth_service_never_lets_the_visitor_through() {
    let w = world(Duration::from_secs(1)).await;
    let r = w.get("slow", "").await;
    assert!(r.starts_with("HTTP/1.1 504"), "{r}");
    *w.up.refuse.lock().unwrap() = Some(Refused::NoClient);
    let r = w.get("alice", "").await;
    assert!(r.starts_with("HTTP/1.1 503"), "{r}");
    *w.up.refuse.lock().unwrap() = Some(Refused::Unreachable);
    let r = w.get("alice", "").await;
    assert!(r.starts_with("HTTP/1.1 502"), "{r}");
    assert_eq!(w.app_calls(), 0);
}

#[tokio::test]
async fn the_auth_service_is_asked_with_the_visitors_address() {
    let w = world(Duration::from_secs(5)).await;
    let _ = w.get("alice", "").await;
    let visitors = w.up.visitors.lock().unwrap().clone();
    let auth = visitors
        .iter()
        .find(|(s, _)| s == "authelia")
        .expect("asked");
    assert_eq!(auth.1.addr.ip().to_string(), "127.0.0.1");
    assert_eq!(auth.1.host, "wiki.example.com");
}
