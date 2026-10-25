mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn blog() -> (burrow_web::Proxy<Fake>, HttpBackend, std::sync::Arc<Fake>) {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let proxy = start(
        |s| {
            s.sites = vec![https_site(
                "blog",
                &["blog.example.com", "*.apps.example.com"],
            )]
        },
        up.clone(),
    )
    .await;
    (proxy, app, up)
}

#[tokio::test]
async fn a_terminated_site_reaches_its_app_with_the_forwarded_headers() {
    let (proxy, _app, up) = blog().await;
    let addr = proxy.https_addr().unwrap();
    let r = over_tls(
        addr,
        "blog.example.com",
        "GET /x?y=1 HTTP/1.1\r\nHost: Blog.Example.com:443\r\nX-Forwarded-For: 6.6.6.6\r\nConnection: close, X-Secret\r\nX-Secret: 1\r\nTE: trailers\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let lower = r.to_ascii_lowercase();
    assert!(lower.contains("get /x?y=1\n"), "{r}");
    assert!(lower.contains("x-forwarded-for: 127.0.0.1\n"), "{r}");
    assert!(lower.contains("x-real-ip: 127.0.0.1\n"), "{r}");
    assert!(lower.contains("x-forwarded-proto: https\n"), "{r}");
    assert!(
        lower.contains("x-forwarded-host: blog.example.com\n"),
        "{r}"
    );
    assert!(lower.contains("x-forwarded-port: 443\n"), "{r}");
    assert!(
        lower.contains("host: blog.example.com:443\n"),
        "Host passes unchanged: {r}"
    );
    assert!(
        !lower.contains("x-secret") && !lower.contains("\nte:"),
        "{r}"
    );
    assert!(lower.contains("x-from-app: yes"), "{r}");
    assert!(
        !lower.contains("keep-alive: timeout"),
        "hop-by-hop answer headers are removed: {r}"
    );
    let seen = up.visitors.lock().unwrap().clone();
    assert_eq!(seen[0].0, "blog");
    assert_eq!(seen[0].1.host, "blog.example.com");
    let r = over_tls(
        addr,
        "x.apps.example.com",
        "GET / HTTP/1.1\r\nHost: x.apps.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 200"), "a wildcard site: {r}");
}

#[tokio::test]
async fn a_host_other_than_the_tls_name_is_421_and_a_missing_or_double_host_is_400() {
    let (proxy, app, up) = blog().await;
    let addr = proxy.https_addr().unwrap();
    for (req, want) in [
        (
            "GET / HTTP/1.1\r\nHost: other.example.com\r\nConnection: close\r\n\r\n",
            "421",
        ),
        (
            "GET / HTTP/1.1\r\nHost: x.apps.example.com\r\nConnection: close\r\n\r\n",
            "421",
        ),
        ("GET / HTTP/1.1\r\nConnection: close\r\n\r\n", "400"),
        (
            "GET / HTTP/1.1\r\nHost: blog.example.com\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n",
            "400",
        ),
        (
            "GET https://other.example.com/ HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n",
            "421",
        ),
    ] {
        let r = over_tls(addr, "blog.example.com", req).await;
        assert!(r.starts_with(&format!("HTTP/1.1 {want}")), "{req:?}: {r}");
        assert!(r.contains("<!doctype html>"), "{r}");
    }
    assert_eq!(
        app.connections.load(Ordering::SeqCst),
        0,
        "no request reached the app"
    );
    assert!(up.visitors.lock().unwrap().is_empty());
}

#[tokio::test]
async fn keep_alive_requests_share_one_stream_to_the_app() {
    let (proxy, app, up) = blog().await;
    let mut t = tls(proxy.https_addr().unwrap(), "blog.example.com")
        .await
        .unwrap();
    for i in 0..20 {
        let req = format!("GET /k{i} HTTP/1.1\r\nHost: blog.example.com\r\n\r\n");
        t.write_all(req.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), t.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let r = String::from_utf8_lossy(&buf[..n]);
        assert!(
            r.starts_with("HTTP/1.1 200") && r.contains(&format!("/k{i}\n")),
            "{r}"
        );
    }
    assert_eq!(app.connections.load(Ordering::SeqCst), 1);
    assert_eq!(up.visitors.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_large_upload_streams_to_the_app() {
    let (proxy, _app, _up) = blog().await;
    let mut t = tls(proxy.https_addr().unwrap(), "blog.example.com")
        .await
        .unwrap();
    let body = vec![b'x'; 8 * 1024 * 1024];
    let head = format!(
        "POST /up HTTP/1.1\r\nHost: blog.example.com\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    t.write_all(head.as_bytes()).await.unwrap();
    t.write_all(&body).await.unwrap();
    let r = exchange(&mut t, b"").await;
    assert!(r.contains("body-len: 8388608"), "{r}");
}

#[tokio::test]
async fn an_upgrade_joins_the_browser_and_the_app() {
    let (proxy, _app, _up) = blog().await;
    let mut t = tls(proxy.https_addr().unwrap(), "blog.example.com")
        .await
        .unwrap();
    t.write_all(b"GET /upgrade HTTP/1.1\r\nHost: blog.example.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n")
        .await
        .unwrap();
    let mut head = Vec::new();
    let mut one = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        t.read_exact(&mut one).await.unwrap();
        head.push(one[0]);
    }
    let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 101"), "{head}");
    assert!(
        head.contains("upgrade: echo") && head.contains("connection: upgrade"),
        "{head}"
    );
    for i in 0..50 {
        let m = format!("m{i}");
        t.write_all(m.as_bytes()).await.unwrap();
        t.flush().await.unwrap();
        let mut got = vec![0u8; m.len()];
        t.read_exact(&mut got).await.unwrap();
        assert_eq!(got, m.as_bytes());
    }
}

#[tokio::test]
async fn a_refusal_before_the_app_answers_is_a_page_on_a_connection_that_stays_usable() {
    let (proxy, _app, up) = blog().await;
    let mut t = tls(proxy.https_addr().unwrap(), "blog.example.com")
        .await
        .unwrap();
    for (why, want) in [
        (burrow_web::Refused::NoClient, "503"),
        (burrow_web::Refused::Full, "503"),
        (burrow_web::Refused::NotAllowed, "403"),
    ] {
        *up.refuse.lock().unwrap() = Some(why);
        t.write_all(b"GET / HTTP/1.1\r\nHost: blog.example.com\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), t.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let r = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
        assert!(r.starts_with(&format!("http/1.1 {want}")), "{why:?}: {r}");
        assert_eq!(
            r.contains("retry-after: 10"),
            why == burrow_web::Refused::Full,
            "{r}"
        );
    }
    *up.refuse.lock().unwrap() = None;
    let r = exchange(
        &mut t,
        b"GET / HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
}

async fn quick_idle() -> (burrow_web::Proxy<Fake>, HttpBackend) {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let proxy = start(
        |s| {
            s.sites = vec![https_site("blog", &["blog.example.com"])];
            s.timeouts.idle = Duration::from_secs(1);
            s.timeouts.request_header = Duration::from_secs(30);
        },
        up.clone(),
    )
    .await;
    (proxy, app)
}

async fn closed_within(t: &mut (impl tokio::io::AsyncRead + Unpin), limit: Duration) -> bool {
    let mut buf = vec![0u8; 4096];
    loop {
        match tokio::time::timeout(limit, t.read(&mut buf)).await {
            Ok(Ok(0) | Err(_)) => return true,
            Ok(Ok(_)) => {}
            Err(_) => return false,
        }
    }
}

#[tokio::test]
async fn a_connection_that_moves_nothing_for_the_idle_time_is_closed() {
    let (proxy, _app) = quick_idle().await;
    let mut t = tls(proxy.https_addr().unwrap(), "blog.example.com")
        .await
        .unwrap();
    t.write_all(b"GET / HTTP/1.1\r\nHost: blog.example.com\r\n\r\n")
        .await
        .unwrap();
    let started = std::time::Instant::now();
    assert!(closed_within(&mut t, Duration::from_secs(4)).await);
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(900) && took < Duration::from_secs(3),
        "{took:?}"
    );
}

#[tokio::test]
async fn an_app_slower_than_the_idle_time_still_answers() {
    let (proxy, _app) = quick_idle().await;
    let r = over_tls(
        proxy.https_addr().unwrap(),
        "blog.example.com",
        "GET /slow/2500 HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
}

#[tokio::test]
async fn an_upgraded_connection_is_not_closed_for_being_quiet() {
    let (proxy, _app) = quick_idle().await;
    let mut t = tls(proxy.https_addr().unwrap(), "blog.example.com")
        .await
        .unwrap();
    t.write_all(b"GET /upgrade HTTP/1.1\r\nHost: blog.example.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n")
        .await
        .unwrap();
    let mut buf = vec![0u8; 4096];
    let n = t.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 101"));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    t.write_all(b"still").await.unwrap();
    t.flush().await.unwrap();
    let mut got = [0u8; 5];
    t.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"still");
}

#[tokio::test]
async fn a_browser_without_a_tls_name_reaches_the_ip_site_it_dialled_over_https() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("kiosk", app.addr);
    let proxy = start(
        |s| s.sites = vec![https_site("kiosk", &["127.0.0.1"])],
        up.clone(),
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let r = over_tls(
        addr,
        "127.0.0.1",
        "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(
        r.contains("x-forwarded-proto: https\n"),
        "served as HTTPS, not plain: {r}"
    );
    let r = over_tls(
        addr,
        "127.0.0.1",
        "GET / HTTP/1.1\r\nHost: other.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 421"), "{r}");
}

#[tokio::test]
async fn stopping_lets_a_request_in_flight_finish_and_closes_idle_keep_alive_connections() {
    let (proxy, _app, _up) = blog().await;
    let addr = proxy.https_addr().unwrap();
    let mut idle = tls(addr, "blog.example.com").await.unwrap();
    idle.write_all(b"GET / HTTP/1.1\r\nHost: blog.example.com\r\n\r\n")
        .await
        .unwrap();
    let mut buf = vec![0u8; 4096];
    let n = idle.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    let mut busy = tls(addr, "blog.example.com").await.unwrap();
    busy.write_all(b"GET /slow/1000 HTTP/1.1\r\nHost: blog.example.com\r\n\r\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let stopping = tokio::spawn(proxy.stop());
    let started = std::time::Instant::now();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), idle.read(&mut buf))
            .await
            .unwrap()
            .unwrap(),
        0,
        "an idle keep-alive connection closes at once"
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    let r = exchange(&mut busy, b"").await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(r.to_ascii_lowercase().contains("connection: close"), "{r}");
    stopping.await.unwrap();
}
