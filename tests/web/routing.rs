use std::net::SocketAddr;
use std::time::Duration;

use burrow::config::settings::{PathRedirect, ServerSettings, SiteOptions};
use burrow::wire::secret::Secret;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::common::*;
use crate::kit::*;
use crate::sites::with_files;

async fn tagged_app(tag: &'static str) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut one = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match s.read(&mut one).await {
                        Ok(1) => head.push(one[0]),
                        _ => return,
                    }
                }
                let text = format!(
                    "app={tag}\n{}",
                    String::from_utf8_lossy(&head).to_ascii_lowercase()
                );
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nServer: app/1\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = s.write_all(reply.as_bytes()).await;
            });
        }
    });
    addr
}

struct Edge {
    _t: Tunnel,
    web: SocketAddr,
    cert: Cert,
    _dir: tempfile::TempDir,
}

const NAMES: [&str; 2] = ["example.com", "www.example.com"];

async fn edge(change: impl FnOnce(&mut ServerSettings, &std::path::Path)) -> Edge {
    let dir = tempfile::tempdir().unwrap();
    let c = cert(&NAMES);
    let secret = Secret::generate();
    let (blog, api, shop) = (
        tagged_app("blog").await,
        tagged_app("api").await,
        tagged_app("shop").await,
    );
    let mut s = web_server(
        &secret,
        &[
            ("blog", site(&["home"], blog), &["example.com"]),
            ("api", site(&["home"], api), &["example.com"]),
            ("shop", site(&["home"], shop), &["example.com"]),
        ],
    );
    for service in ["blog", "api", "shop"] {
        with_files(&mut s, service, &c, dir.path());
    }
    let web = s.web.as_mut().unwrap();
    web.sites.get_mut("api").unwrap().options = SiteOptions {
        paths: vec!["/api".into()],
        app_path: Some("/".into()),
        max_body_size: Some(10),
        redirects: vec![PathRedirect {
            from: "/api/old".into(),
            to: "/api/new".into(),
            permanent: false,
        }],
        ..SiteOptions::default()
    };
    web.sites.get_mut("shop").unwrap().options = SiteOptions {
        paths: vec!["/shop".into()],
        maintenance: true,
        error_pages: Some(dir.path().to_path_buf()),
        ..SiteOptions::default()
    };
    web.sites.get_mut("blog").unwrap().options = SiteOptions {
        redirect_from: vec!["www.example.com".into()],
        hsts: true,
        response_headers: vec![
            ("X-Frame-Options".into(), "DENY".into()),
            ("Server".into(), String::new()),
        ],
        request_headers: vec![("X-From-Burrow".into(), "yes".into())],
        ..SiteOptions::default()
    };
    std::fs::write(dir.path().join("maintenance.html"), "<p>shop is closed</p>").unwrap();
    change(&mut s, dir.path());
    let t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let web = t.server.web_addr.unwrap();
    Edge {
        _t: t,
        web,
        cert: c,
        _dir: dir,
    }
}

async fn send(e: &Edge, name: &str, req: &str) -> String {
    let mut t = visit(e.web, name, &e.cert).await.unwrap();
    let _ = t.write_all(req.as_bytes()).await;
    let _ = t.flush().await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), t.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

async fn get(e: &Edge, target: &str, extra: &str) -> String {
    send(
        e,
        "example.com",
        &format!("GET {target} HTTP/1.1\r\nHost: example.com\r\n{extra}Connection: close\r\n\r\n"),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_real_server_routes_by_path_rewrites_app_path_and_redirects() {
    let e = edge(|_, _| {}).await;
    let r = get(&e, "//api//users?x=1", "X-Forwarded-Prefix: /evil\r\n").await;
    assert!(r.contains("app=api\n"), "{r}");
    assert!(r.contains("get /users?x=1 http/1.1\r\n"), "{r}");
    assert!(r.contains("x-forwarded-prefix: /api\r\n"), "{r}");
    assert!(!r.contains("/evil"), "{r}");
    let r = get(&e, "/apix", "").await;
    assert!(r.contains("app=blog\n"), "{r}");
    let r = get(&e, "/api/old/page?q=1", "").await;
    assert!(r.starts_with("HTTP/1.1 307"), "{r}");
    assert!(
        r.to_ascii_lowercase()
            .contains("location: /api/new/page?q=1\r\n"),
        "{r}"
    );
    let r = send(
        &e,
        "www.example.com",
        "GET /a?b HTTP/1.1\r\nHost: www.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 308"), "{r}");
    assert!(
        r.to_ascii_lowercase()
            .contains("location: https://example.com/a?b\r\n"),
        "{r}"
    );
    let r = get(&e, "/public%2F..%2Fadmin", "").await;
    assert!(r.starts_with("HTTP/1.1 400"), "{r}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_real_server_applies_limits_pages_maintenance_header_rules_and_hsts() {
    let e = edge(|_, _| {}).await;
    let r = send(
        &e,
        "example.com",
        "POST /api/up HTTP/1.1\r\nHost: example.com\r\nContent-Length: 11\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 413"), "{r}");
    assert!(!r.contains("100 Continue"), "{r}");
    let r = get(&e, "/shop/cart", "").await;
    assert!(r.starts_with("HTTP/1.1 503"), "{r}");
    assert!(r.contains("<p>shop is closed</p>"), "{r}");
    assert!(!r.contains("app=shop"), "{r}");
    let r = get(&e, "/", "").await;
    let lower = r.to_ascii_lowercase();
    assert!(r.contains("app=blog\n"), "{r}");
    assert!(r.contains("x-from-burrow: yes\r\n"), "{r}");
    assert!(lower.contains("x-frame-options: deny\r\n"), "{r}");
    assert!(!lower.contains("server: app/1"), "{r}");
    assert!(
        lower.contains("strict-transport-security: max-age=31536000\r\n"),
        "{r}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_real_server_strips_a_visitors_forwarded_headers_and_trusts_only_its_proxies() {
    let e = edge(|_, _| {}).await;
    let spoof = "X-Forwarded-For: 6.6.6.6\r\nForwarded: for=6.6.6.6\r\nX_Real_IP: 6.6.6.6\r\nX-Forwarded-Host: evil.example.com\r\nConnection: close, X-Real-IP\r\n";
    let r = send(
        &e,
        "example.com",
        &format!("GET / HTTP/1.1\r\nHost: example.com\r\n{spoof}\r\n"),
    )
    .await;
    assert!(r.contains("x-forwarded-for: 127.0.0.1\r\n"), "{r}");
    assert!(r.contains("x-real-ip: 127.0.0.1\r\n"), "{r}");
    assert!(r.contains("x-forwarded-host: example.com\r\n"), "{r}");
    assert!(
        !r.contains("6.6.6.6") && !r.contains("forwarded: for"),
        "{r}"
    );
    let trusted = edge(|s, _| {
        s.web.as_mut().unwrap().trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
    })
    .await;
    let r = get(
        &trusted,
        "/",
        "X-Forwarded-For: 203.0.113.9\r\nX-Forwarded-Proto: https\r\n",
    )
    .await;
    assert!(r.contains("x-real-ip: 203.0.113.9\r\n"), "{r}");
    assert!(
        r.contains("x-forwarded-for: 203.0.113.9, 127.0.0.1\r\n"),
        "{r}"
    );
}
