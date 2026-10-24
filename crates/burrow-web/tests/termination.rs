mod common;

use std::net::SocketAddr;

use common::*;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_websockets::{ClientBuilder, Message, ServerBuilder};

async fn ws_echo() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            tokio::spawn(async move {
                let Ok((_, mut ws)) = ServerBuilder::new().accept(s).await else {
                    return;
                };
                while let Some(Ok(m)) = ws.next().await {
                    if m.is_close() || ws.send(m).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

#[tokio::test]
async fn a_websocket_passes_through_termination_with_small_and_large_frames() {
    let app = ws_echo().await;
    let (up, _) = Fake::new(4);
    up.serve("chat", app);
    let proxy = start(
        |s| s.sites = vec![https_site("chat", &["*.apps.example.com"])],
        up,
    )
    .await;
    let t = tls(proxy.https_addr().unwrap(), "chat.apps.example.com")
        .await
        .unwrap();
    let (mut ws, answer) = ClientBuilder::new()
        .uri("wss://chat.apps.example.com/socket")
        .unwrap()
        .connect_on(t)
        .await
        .unwrap();
    assert_eq!(answer.status(), 101);
    for i in 0..200 {
        let m = format!("m{i}");
        ws.send(Message::text(m.clone())).await.unwrap();
        let got = ws.next().await.unwrap().unwrap();
        assert_eq!(got.as_text(), Some(m.as_str()));
    }
    let big = vec![7u8; 1024 * 1024];
    ws.send(Message::binary(big.clone())).await.unwrap();
    let got = ws.next().await.unwrap().unwrap();
    assert_eq!(&*got.into_payload(), big.as_slice());
    ws.close().await.unwrap();
}

#[tokio::test]
async fn one_port_passes_one_name_through_and_terminates_another() {
    let (nas, mut sni) = backend("nas", &["nas.example.com"]).await;
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("nas", nas);
    up.serve("blog", app.addr);
    let proxy = start(
        |s| {
            s.sites = vec![
                site("nas", &["nas.example.com"]),
                https_site("blog", &["blog.example.com"]),
            ]
        },
        up,
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let mut t = tls(addr, "nas.example.com").await.unwrap();
    assert_eq!(ask(&mut t, b"ping").await.unwrap(), b"nas:ping");
    assert_eq!(
        sni.recv().await.unwrap().as_deref(),
        Some("nas.example.com"),
        "the app's own TLS saw the visitor's hello"
    );
    let r = over_tls(
        addr,
        "blog.example.com",
        "GET /after-peek HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        r.starts_with("HTTP/1.1 200") && r.contains("GET /after-peek\n"),
        "{r}"
    );
}

#[tokio::test]
async fn a_terminated_name_added_live_is_served_while_an_open_connection_keeps_working() {
    let (blog, wiki) = (http_backend().await, http_backend().await);
    let (up, _) = Fake::new(4);
    up.serve("blog", blog.addr);
    up.serve("wiki", wiki.addr);
    let proxy = start(
        |s| s.sites = vec![https_site("blog", &["blog.example.com"])],
        up,
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let get = |host: &str, path: &str| format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n");
    let mut kept = tls(addr, "blog.example.com").await.unwrap();
    let r = ask(&mut kept, get("blog.example.com", "/1").as_bytes())
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&r).contains("GET /1\n"));
    let mut next = burrow_web::ProxySettings::new(addr);
    next.sites = vec![
        https_site("blog", &["blog.example.com"]),
        https_site("wiki", &["wiki.example.com"]),
    ];
    proxy.apply(&next).await;
    let r = over_tls(
        addr,
        "wiki.example.com",
        "GET /n HTTP/1.1\r\nHost: wiki.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        r.starts_with("HTTP/1.1 200") && r.contains("GET /n\n"),
        "{r}"
    );
    let r = ask(&mut kept, get("blog.example.com", "/2").as_bytes())
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&r).contains("GET /2\n"));
    assert_eq!(
        wiki.connections.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}
