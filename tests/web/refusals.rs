use std::net::SocketAddr;

use burrow::wire::secret::Secret;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::common::*;
use crate::kit::*;

async fn closed_port() -> SocketAddr {
    free_addr()
}

async fn dying_app() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\npartial")
                    .await;
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let _ = socket2::SockRef::from(&s).set_linger(Some(std::time::Duration::ZERO));
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_refusal_before_the_app_answers_is_its_page_with_a_normal_close() {
    let app = http_app().await;
    let secret = Secret::generate();
    let mut denied = site(&["home"], app);
    denied.sources.allow = vec!["10.0.0.0/8".parse().unwrap()];
    let mut limited = site(&["home"], app);
    limited.max_connections = Some(1);
    let s = plain_server(
        &secret,
        &[
            ("away", site(&["nobody"], app), &["away.example.com"]),
            (
                "gone",
                site(&["home"], closed_port().await),
                &["gone.example.com"],
            ),
            ("lan", denied, &["lan.example.com"]),
            ("one", limited, &["one.example.com"]),
        ],
    );
    let http = s.web.as_ref().unwrap().http.unwrap();
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    for (host, want) in [
        ("away.example.com", "503"),
        ("gone.example.com", "502"),
        ("lan.example.com", "403"),
    ] {
        let r = plain_get(http, host, "/").await;
        assert!(r.starts_with(&format!("HTTP/1.1 {want}")), "{host}: {r}");
        assert!(r.ends_with("</html>\n"), "{host}: {r}");
    }
    let mut first = TcpStream::connect(http).await.unwrap();
    let r = ask(
        &mut first,
        b"GET / HTTP/1.1\r\nHost: one.example.com\r\n\r\n",
    )
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&r).starts_with("HTTP/1.1 200"));
    let r = plain_get(http, "one.example.com", "/").await;
    assert!(r.starts_with("HTTP/1.1 503"), "max_connections: 1: {r}");
    assert!(
        r.to_ascii_lowercase().contains("retry-after: 10\r\n"),
        "{r}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_that_fails_after_its_answer_started_resets_the_browser_connection() {
    let secret = Secret::generate();
    let s = plain_server(
        &secret,
        &[(
            "shaky",
            site(&["home"], dying_app().await),
            &["shaky.example.com"],
        )],
    );
    let http = s.web.as_ref().unwrap().http.unwrap();
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut c = TcpStream::connect(http).await.unwrap();
    c.write_all(b"GET / HTTP/1.1\r\nHost: shaky.example.com\r\n\r\n")
        .await
        .unwrap();
    assert!(reset_quickly(&mut c).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_at_its_connection_limit_gives_the_busy_page_not_a_bad_gateway() {
    let app = http_app().await;
    let secret = Secret::generate();
    let mut s = plain_server(
        &secret,
        &[
            ("a", site(&["home"], app), &["a.example.com"]),
            ("b", site(&["home"], app), &["b.example.com"]),
        ],
    );
    s.max_connections_per_client = 1;
    let http = s.web.as_ref().unwrap().http.unwrap();
    let _t = tunnel_with(s, Vec::new(), |c| c, secret).await;
    let mut first = TcpStream::connect(http).await.unwrap();
    let r = ask(&mut first, b"GET / HTTP/1.1\r\nHost: a.example.com\r\n\r\n")
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&r).starts_with("HTTP/1.1 200"));
    let r = plain_get(http, "b.example.com", "/").await;
    assert!(r.starts_with("HTTP/1.1 503"), "{r}");
    assert!(
        r.to_ascii_lowercase().contains("retry-after: 10\r\n"),
        "{r}"
    );
}
