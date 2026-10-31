mod common;

use std::time::{Duration, Instant};

use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn setup(header_timeout: Duration) -> (burrow_web::Proxy<Fake>, HttpBackend) {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let proxy = start(
        |s| {
            s.sites = vec![https_site("blog", &["blog.example.com"])];
            s.timeouts.request_header = header_timeout;
        },
        up,
    )
    .await;
    (proxy, app)
}

async fn raw(proxy: &burrow_web::Proxy<Fake>, req: &[u8]) -> String {
    let mut t = tls(proxy.https_addr().unwrap(), "blog.example.com")
        .await
        .unwrap();
    let _ = t.write_all(req).await;
    let _ = t.flush().await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), t.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn a_request_hidden_in_a_body_never_reaches_the_app() {
    let (proxy, _app) = setup(Duration::from_secs(10)).await;
    let r = raw(
        &proxy,
        b"POST / HTTP/1.1\r\nHost: blog.example.com\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\nGET /smuggled HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    let lower = r.to_ascii_lowercase();
    assert!(lower.starts_with("http/1.1 200"), "{r}");
    assert!(
        lower.starts_with("http/1.1 200")
            && lower.contains("\r\n\r\npost /\n")
            && lower.contains("body-len: 0\n"),
        "CL+TE is read as chunked: {r}"
    );
    assert!(
        !lower.contains("content-length: 4\n"),
        "the conflicting length is dropped: {r}"
    );
    assert_eq!(
        lower.matches("transfer-encoding: chunked\n").count(),
        1,
        "the app gets burrow's own framing: {r}"
    );
    for (req, what) in [
        (
            &b"POST / HTTP/1.1\r\nHost: blog.example.com\r\nContent-Length: 4\r\nContent-Length: 5\r\n\r\nabcde"[..],
            "CL,CL",
        ),
        (
            &b"POST / HTTP/1.1\r\nHost: blog.example.com\r\nTransfer-Encoding: xchunked\r\n\r\n0\r\n\r\n"[..],
            "TE xchunked",
        ),
        (
            &b"POST / HTTP/1.0\r\nHost: blog.example.com\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"[..],
            "TE on HTTP/1.0",
        ),
        (&b"GET / HTTP/1.1\r\nHost: blog.example.com\r\nX-A: 1\r\n  folded\r\n\r\n"[..], "obs-fold"),
    ] {
        let r = raw(&proxy, req).await;
        assert!(r.starts_with("HTTP/1.1 400"), "{what}: {r}");
    }
}

#[tokio::test]
async fn heads_over_32_kib_or_100_headers_get_431() {
    let (proxy, app) = setup(Duration::from_secs(10)).await;
    let mut big = b"GET / HTTP/1.1\r\nHost: blog.example.com\r\nX-Big: ".to_vec();
    big.extend(vec![b'a'; 33 * 1024]);
    big.extend(b"\r\n\r\n");
    let r = raw(&proxy, &big).await;
    assert!(
        r.starts_with("HTTP/1.1 431"),
        "{}",
        r.lines().next().unwrap_or("(closed)")
    );
    let mut many = b"GET / HTTP/1.1\r\nHost: blog.example.com\r\n".to_vec();
    for i in 0..101 {
        many.extend(format!("X-{i}: 1\r\n").as_bytes());
    }
    many.extend(b"\r\n");
    let r = raw(&proxy, &many).await;
    assert!(
        r.starts_with("HTTP/1.1 431"),
        "{}",
        r.lines().next().unwrap_or("(closed)")
    );
    let mut fine =
        b"GET / HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\nX-Big: ".to_vec();
    fine.extend(vec![b'a'; 30 * 1024]);
    fine.extend(b"\r\n\r\n");
    let r = raw(&proxy, &fine).await;
    assert!(
        r.starts_with("HTTP/1.1 200"),
        "{}",
        r.lines().next().unwrap_or("(closed)")
    );
    assert_eq!(app.connections.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_head_sent_too_slowly_is_closed_after_the_header_timeout() {
    let (proxy, _app) = setup(Duration::from_secs(1)).await;
    let mut t = tls(proxy.https_addr().unwrap(), "blog.example.com")
        .await
        .unwrap();
    t.write_all(b"GET / HTTP/1.1\r\nHost: blog.").await.unwrap();
    t.flush().await.unwrap();
    let started = Instant::now();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), t.read_to_end(&mut out)).await;
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(900) && took < Duration::from_secs(3),
        "{took:?}"
    );
}
