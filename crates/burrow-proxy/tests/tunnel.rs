mod common;

use std::time::Duration;

use burrow_proxy::{TunnelMode, TunnelSettings};
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn tunnel(host: &str, mode: TunnelMode) -> Option<TunnelSettings> {
    Some(TunnelSettings {
        host: host.into(),
        path: "/burrow".into(),
        mode,
        tls: Some(acceptor(&[host])),
    })
}

const UPGRADE: &str = "GET /burrow HTTP/1.1\r\nHost: site.example.com\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";

async fn head<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> String {
    let mut got = Vec::new();
    let mut one = [0u8; 1];
    while !got.ends_with(b"\r\n\r\n") {
        match tokio::time::timeout(Duration::from_secs(5), s.read(&mut one)).await {
            Ok(Ok(1)) => got.push(one[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&got).into_owned()
}

#[tokio::test]
async fn the_tunnel_on_its_own_name_gets_the_tls_stream_beside_passthrough_sites() {
    let (nas, _n) = backend("nas", &["nas.example.com"]).await;
    let (up, mut tunneled) = Fake::new(4);
    up.serve("nas", nas);
    let proxy = start(
        |s| {
            s.sites = vec![site("nas", &["nas.example.com"])];
            s.tunnel = tunnel("tunnel.example.com", TunnelMode::OwnName);
        },
        up.clone(),
    )
    .await;
    let addr = proxy.local_addr();
    let mut t = tls(addr, "tunnel.example.com").await.unwrap();
    assert_eq!(ask(&mut t, b"noise").await.unwrap(), b"tunnel:noise");
    let seen = tunneled.recv().await.unwrap();
    assert!(!seen.upgraded);
    assert_eq!(seen.path, "/burrow");
    assert_eq!(seen.local, addr);
    assert_eq!(seen.peer.ip(), addr.ip());
    let mut n = tls(addr, "nas.example.com").await.unwrap();
    assert_eq!(ask(&mut n, b"x").await.unwrap(), b"nas:x");
}

#[tokio::test]
async fn the_tunnel_by_path_answers_the_upgrade_and_hands_over_the_rest() {
    let (up, mut tunneled) = Fake::new(4);
    let proxy = start(
        |s| s.tunnel = tunnel("site.example.com", TunnelMode::ByPath),
        up.clone(),
    )
    .await;
    let addr = proxy.local_addr();
    let mut t = tls(addr, "site.example.com").await.unwrap();
    let mut first = UPGRADE.as_bytes().to_vec();
    first.extend_from_slice(b"early");
    t.write_all(&first).await.unwrap();
    let h = head(&mut t).await;
    assert!(h.starts_with("HTTP/1.1 101"), "{h}");
    assert!(
        h.to_ascii_lowercase()
            .contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
        "{h}"
    );
    let mut rest = Vec::new();
    t.read_to_end(&mut rest).await.unwrap();
    assert_eq!(
        rest, b"tunnel:early",
        "bytes sent with the request reach the tunnel"
    );
    assert!(tunneled.recv().await.unwrap().upgraded);
}

#[tokio::test]
async fn other_requests_on_a_tunnel_name_get_404_or_426() {
    let (up, _t) = Fake::new(4);
    let proxy = start(
        |s| s.tunnel = tunnel("site.example.com", TunnelMode::ByPath),
        up.clone(),
    )
    .await;
    let addr = proxy.local_addr();
    for (req, want) in [
        ("GET / HTTP/1.1\r\nHost: site.example.com\r\n\r\n", "404"),
        (
            "GET /burrow HTTP/1.1\r\nHost: site.example.com\r\n\r\n",
            "426",
        ),
        (
            "GET /burrow/x HTTP/1.1\r\nHost: site.example.com\r\n\r\n",
            "426",
        ),
        (
            "GET /wrong HTTP/1.1\r\nHost: site.example.com\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
            "404",
        ),
    ] {
        let mut t = tls(addr, "site.example.com").await.unwrap();
        t.write_all(req.as_bytes()).await.unwrap();
        let h = head(&mut t).await;
        assert!(h.starts_with(&format!("HTTP/1.1 {want}")), "{req:?}: {h}");
    }
    let mut big = tls(addr, "site.example.com").await.unwrap();
    let huge = format!(
        "GET /burrow HTTP/1.1\r\nHost: a\r\nX-Fill: {}\r\n\r\n",
        "a".repeat(9000)
    );
    let _ = big.write_all(huge.as_bytes()).await;
    let h = head(&mut big).await;
    assert!(h.is_empty() || h.starts_with("HTTP/1.1 431"), "{h}");
}

#[tokio::test]
async fn a_flood_of_passthrough_connections_does_not_block_tunnel_handshakes() {
    let (nas, _n) = backend("nas", &["nas.example.com"]).await;
    let (up, _t) = Fake::new(2);
    up.serve("nas", nas);
    let proxy = start(
        |s| {
            s.sites = vec![site("nas", &["nas.example.com"])];
            s.tunnel = tunnel("tunnel.example.com", TunnelMode::OwnName);
            s.max_pending = 4;
        },
        up.clone(),
    )
    .await;
    let addr = proxy.local_addr();
    let mut open = Vec::new();
    for i in 0..40 {
        let mut c = tls(addr, "nas.example.com").await.unwrap();
        assert_eq!(
            ask(&mut c, b"p").await.unwrap(),
            b"nas:p",
            "passthrough {i}"
        );
        open.push(c);
    }
    let mut t = tls(addr, "tunnel.example.com").await.unwrap();
    assert_eq!(ask(&mut t, b"n").await.unwrap(), b"tunnel:n");
}

#[tokio::test]
async fn a_flood_of_stalled_tunnel_handshakes_does_not_block_passthrough() {
    let (nas, _n) = backend("nas", &["nas.example.com"]).await;
    let (up, _t) = Fake::new(2);
    up.serve("nas", nas);
    let proxy = start(
        |s| {
            s.sites = vec![site("nas", &["nas.example.com"])];
            s.tunnel = tunnel("tunnel.example.com", TunnelMode::OwnName);
            s.max_pending = 4;
        },
        up.clone(),
    )
    .await;
    let addr = proxy.local_addr();
    let mut stalled = Vec::new();
    for _ in 0..8 {
        stalled.push(tls(addr, "tunnel.example.com").await);
    }
    assert!(
        stalled.iter().filter(|s| s.is_ok()).count() <= 2,
        "the tunnel's own pool caps its handshakes"
    );
    for _ in 0..10 {
        let mut c = tls(addr, "nas.example.com").await.unwrap();
        assert_eq!(ask(&mut c, b"q").await.unwrap(), b"nas:q");
    }
}

#[tokio::test]
async fn a_tunnel_handshake_that_outlasts_its_deadline_is_closed() {
    let (up, _t) = Fake::new(4);
    let proxy = start(
        |s| {
            s.tunnel = tunnel("tunnel.example.com", TunnelMode::OwnName);
            s.handshake_timeout = Duration::from_millis(300);
        },
        up.clone(),
    )
    .await;
    let mut raw = TcpStream::connect(proxy.local_addr()).await.unwrap();
    let hello = {
        let mut half = Vec::new();
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            let _ = connector()
                .connect(
                    tokio_rustls::rustls::pki_types::ServerName::try_from("tunnel.example.com")
                        .unwrap(),
                    &mut a,
                )
                .await;
        });
        let mut buf = [0u8; 4096];
        let n = b.read(&mut buf).await.unwrap();
        half.extend_from_slice(&buf[..n]);
        half
    };
    raw.write_all(&hello).await.unwrap();
    let mut buf = vec![0u8; 8192];
    let started = std::time::Instant::now();
    loop {
        match tokio::time::timeout(Duration::from_secs(3), raw.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => break,
            Ok(Ok(_)) => continue,
            Err(_) => panic!("still open after 3 s"),
        }
    }
    assert!(started.elapsed() < Duration::from_secs(2));
}
