mod common;

use std::time::Duration;

use burrow_web::Refused;
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn setup() -> (burrow_web::Proxy<Fake>, std::sync::Arc<Fake>) {
    let (up, _) = Fake::new(4);
    let proxy = start_with(
        |s| {
            s.sites = vec![
                plain_site("wiki", &["wiki.example.com"]),
                https_site("blog", &["blog.example.com"]),
            ]
        },
        up.clone(),
        fixed(&["blog.example.com"]),
        true,
    )
    .await;
    (proxy, up)
}

const GET: &[u8] = b"GET / HTTP/1.1\r\nHost: wiki.example.com\r\nConnection: close\r\n\r\n";

async fn read_all(s: &mut TcpStream) -> (String, std::io::Result<()>) {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    let end = loop {
        match tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf)).await {
            Ok(Ok(0)) => break Ok(()),
            Ok(Ok(n)) => out.extend_from_slice(&buf[..n]),
            Ok(Err(e)) => break Err(e),
            Err(_) => break Err(std::io::ErrorKind::TimedOut.into()),
        }
    };
    (String::from_utf8_lossy(&out).into_owned(), end)
}

#[tokio::test]
async fn a_refusal_after_the_stream_opened_but_before_the_answer_is_its_page_with_a_clean_close() {
    let (proxy, up) = setup().await;
    let http = proxy.http_addr().unwrap();
    for (why, want) in [
        (Refused::Down, "503"),
        (Refused::Full, "503"),
        (Refused::NoClient, "503"),
        (Refused::Denied, "502"),
        (Refused::Unreachable, "502"),
        (Refused::NotAllowed, "403"),
    ] {
        *up.script.lock().unwrap() = Some(Script::RefuseLater(why));
        let mut s = TcpStream::connect(http).await.unwrap();
        s.write_all(GET).await.unwrap();
        let (r, end) = read_all(&mut s).await;
        assert!(r.starts_with(&format!("HTTP/1.1 {want}")), "{why:?}: {r}");
        assert!(r.ends_with("</html>\n"), "the whole page arrives: {r}");
        assert!(end.is_ok(), "{why:?}: closed normally, not reset: {end:?}");
    }
}

#[tokio::test]
async fn an_app_that_resets_before_answering_gives_502() {
    let (proxy, up) = setup().await;
    *up.script.lock().unwrap() = Some(Script::Reset);
    let mut s = TcpStream::connect(proxy.http_addr().unwrap())
        .await
        .unwrap();
    s.write_all(GET).await.unwrap();
    let (r, end) = read_all(&mut s).await;
    assert!(
        r.starts_with("HTTP/1.1 404") || r.starts_with("HTTP/1.1 502"),
        "{r}"
    );
    assert!(end.is_ok(), "{end:?}");
}

#[tokio::test]
async fn an_app_that_fails_after_its_answer_started_resets_the_browser_connection() {
    let (proxy, up) = setup().await;
    *up.script.lock().unwrap() = Some(Script::DieMidResponse);
    let mut s = TcpStream::connect(proxy.http_addr().unwrap())
        .await
        .unwrap();
    s.write_all(GET).await.unwrap();
    let (r, end) = read_all(&mut s).await;
    assert!(
        r.starts_with("HTTP/1.1 200") && r.ends_with("partial"),
        "{r}"
    );
    assert_eq!(end.unwrap_err().kind(), std::io::ErrorKind::ConnectionReset);
    let mut t = tls(proxy.https_addr().unwrap(), "blog.example.com")
        .await
        .unwrap();
    t.write_all(b"GET / HTTP/1.1\r\nHost: blog.example.com\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    let got = tokio::time::timeout(Duration::from_secs(5), t.read_to_end(&mut out))
        .await
        .unwrap();
    assert!(
        got.is_err(),
        "over TLS too the connection ends without close_notify: {got:?}"
    );
}
