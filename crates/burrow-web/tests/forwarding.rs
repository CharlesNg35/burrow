mod common;

use std::sync::Arc;
use std::time::Duration;

use burrow_web::{AccessLog, LogFormat, Site, SiteOptions};
use common::*;
use tokio::net::TcpListener;

fn trusted() -> Vec<burrow_syntax::cidr::Cidr> {
    vec!["127.0.0.1/32".parse().unwrap()]
}

async fn get(addr: std::net::SocketAddr, target: &str, extra: &str) -> String {
    over_tls(
        addr,
        "example.com",
        &format!("GET {target} HTTP/1.1\r\nHost: example.com\r\n{extra}Connection: close\r\n\r\n"),
    )
    .await
}

#[tokio::test]
async fn behind_a_trusted_proxy_the_visitor_comes_from_x_forwarded_for() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let proxy = start(
        |s| {
            s.sites = vec![https_site("blog", &["example.com"])];
            s.forwarding.trusted_proxies = trusted();
        },
        up.clone(),
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let r = get(
        addr,
        "/",
        "X-Forwarded-For: 6.6.6.6, 203.0.113.9\r\nX-Forwarded-Proto: http\r\nX-Forwarded-Host: public.example.com\r\n",
    )
    .await;
    assert!(r.contains("x-real-ip: 203.0.113.9\n"), "{r}");
    assert!(
        r.contains("x-forwarded-for: 6.6.6.6, 203.0.113.9, 127.0.0.1\n"),
        "{r}"
    );
    assert!(r.contains("x-forwarded-proto: http\n"), "{r}");
    assert!(r.contains("x-forwarded-host: public.example.com\n"), "{r}");
    assert!(r.contains("x-forwarded-port: 80\n"), "{r}");
    let seen = up.visitors.lock().unwrap()[0].1.addr.ip();
    assert_eq!(seen, "203.0.113.9".parse::<std::net::IpAddr>().unwrap());
}

#[tokio::test]
async fn a_plain_site_behind_a_trusted_proxy_redirects_by_its_x_forwarded_proto() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let proxy = start_with(
        |s| {
            s.sites = vec![Site {
                https_redirect: true,
                ..plain_site("blog", &["example.com"])
            }];
            s.forwarding.trusted_proxies = trusted();
        },
        up,
        fixed(&["example.com"]),
        true,
    )
    .await;
    let http = proxy.http_addr().unwrap();
    let ask = |proto: &str| {
        format!(
            "GET /a?b HTTP/1.1\r\nHost: example.com\r\nX-Forwarded-Proto: {proto}\r\nConnection: close\r\n\r\n"
        )
    };
    let r = over_plain(http, &ask("https")).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(r.contains("x-forwarded-proto: https\n"), "{r}");
    let r = over_plain(http, &ask("http")).await;
    assert!(r.starts_with("HTTP/1.1 308"), "{r}");
    assert!(r.contains("location: https://example.com/a?b\r\n"), "{r}");
    assert!(!r.contains("strict-transport-security"), "{r}");
}

#[tokio::test]
async fn request_ids_reach_the_app_the_visitor_and_the_access_log() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("access.log");
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let file = log.clone();
    let proxy = start(
        move |s| {
            s.sites = vec![https_site("blog", &["example.com"])];
            s.request_id = true;
            s.access_log = Some(AccessLog {
                file,
                format: LogFormat::Json,
            });
        },
        up,
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let r = get(addr, "/a\"b?q=1", "X-Request-Id: evil\r\n").await;
    let id = r
        .lines()
        .find_map(|l| l.strip_prefix("x-request-id: "))
        .unwrap()
        .trim()
        .to_string();
    assert_eq!(id.len(), 16, "{r}");
    assert!(!r.contains("evil"), "{r}");
    assert!(r.contains(&format!("x-request-id: {id}\n")), "{r}");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let text = loop {
        let t = std::fs::read_to_string(&log).unwrap_or_default();
        if !t.is_empty() || std::time::Instant::now() > deadline {
            break t;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let v: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(v["request_id"], id.as_str());
    assert_eq!(v["path"], "/a%22b?q=1");
    assert_eq!(v["status"], 200);
    assert_eq!(v["service"], "blog");
    assert_eq!(v["host"], "example.com");
    assert_eq!(v["client"], "127.0.0.1");
    assert!(v["bytes"].as_u64().unwrap() > 0);
}

async fn html_app() -> std::net::SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(
                    |req: hyper::Request<hyper::body::Incoming>| async move {
                        let mut r = hyper::Response::new(http_body_util::Full::new(
                            bytes::Bytes::from("<p>hello</p>".repeat(200)),
                        ));
                        let h = r.headers_mut();
                        h.insert("content-type", "text/html".parse().unwrap());
                        h.insert("etag", "\"v1\"".parse().unwrap());
                        if req.uri().path() == "/raw" {
                            h.insert("cache-control", "no-transform".parse().unwrap());
                        }
                        Ok::<_, std::convert::Infallible>(r)
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn compress_gzips_what_the_client_accepts_and_leaves_the_rest() {
    let (up, _) = Fake::new(4);
    up.serve("blog", html_app().await);
    let options = SiteOptions {
        compress: true,
        ..SiteOptions::default()
    };
    let proxy = start(
        |s| {
            s.sites = vec![Site {
                options,
                ..https_site("blog", &["example.com"])
            }]
        },
        Arc::clone(&up),
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let r = get(addr, "/", "Accept-Encoding: gzip, br\r\n").await;
    assert!(r.contains("content-encoding: gzip\r\n"), "{r}");
    assert!(r.contains("vary: accept-encoding\r\n"), "{r}");
    assert!(r.contains("etag: W/\"v1\"\r\n"), "{r}");
    assert!(!r.contains("<p>hello</p>"), "{r}");
    let r = get(addr, "/", "Accept-Encoding: gzip;q=0\r\n").await;
    assert!(!r.contains("content-encoding"), "{r}");
    assert!(r.contains("<p>hello</p>"), "{r}");
    let r = get(addr, "/raw", "Accept-Encoding: gzip\r\n").await;
    assert!(!r.contains("content-encoding"), "{r}");
    let head = over_tls(
        addr,
        "example.com",
        "HEAD / HTTP/1.1\r\nHost: example.com\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(!head.contains("content-encoding"), "{head}");
}

#[tokio::test]
async fn one_proxy_connection_carrying_two_visitors_checks_each_of_them() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    up.denied
        .lock()
        .unwrap()
        .push("198.51.100.66".parse().unwrap());
    let proxy = start(
        |s| {
            s.sites = vec![https_site("blog", &["example.com"])];
            s.forwarding.trusted_proxies = trusted();
        },
        up.clone(),
    )
    .await;
    let mut t = tls(proxy.https_addr().unwrap(), "example.com")
        .await
        .unwrap();
    let ask =
        |ip: &str| format!("GET / HTTP/1.1\r\nHost: example.com\r\nX-Forwarded-For: {ip}\r\n\r\n");
    let first = read_response(&mut t, &ask("203.0.113.9")).await;
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    assert!(first.contains("x-real-ip: 203.0.113.9\n"), "{first}");
    let second = read_response(&mut t, &ask("198.51.100.66")).await;
    assert!(second.starts_with("HTTP/1.1 403"), "{second}");
    let third = read_response(&mut t, &ask("203.0.113.10")).await;
    assert!(third.contains("x-real-ip: 203.0.113.10\n"), "{third}");
    assert_eq!(app.connections.load(std::sync::atomic::Ordering::SeqCst), 2);
}
