mod common;

use std::sync::Arc;
use std::time::Duration;

use burrow_web::{TunnelMode, TunnelPort, TunnelSettings};
use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

struct Setup {
    proxy: burrow_web::Proxy<Fake>,
    certs: Arc<Scripted>,
    tunneled: mpsc::UnboundedReceiver<Tunneled>,
}

async fn on_port_80(host: &str, mode: TunnelMode) -> Setup {
    let app = http_backend().await;
    let (up, tunneled) = Fake::new(4);
    up.serve("wiki", app.addr);
    up.serve("shop", app.addr);
    let certs = Scripted::new();
    let proxy = start_with(
        |s| {
            s.sites = vec![
                plain_site("wiki", &["wiki.example.com"]),
                https_site("shop", &["shop.example.com"]),
            ];
            s.tunnel = Some(TunnelSettings {
                host: host.into(),
                path: "/burrow".into(),
                mode,
                port: TunnelPort::Http,
            });
        },
        up,
        certs.clone(),
        true,
    )
    .await;
    Setup {
        proxy,
        certs,
        tunneled,
    }
}

fn upgrade(host: &str) -> Vec<u8> {
    format!("GET /burrow HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\nearly").into_bytes()
}

async fn upgraded(s: &mut Setup, host: &str) {
    let http = s.proxy.http_addr().unwrap();
    let mut t = TcpStream::connect(http).await.unwrap();
    t.write_all(&upgrade(host)).await.unwrap();
    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), t.read_to_end(&mut got))
        .await
        .unwrap()
        .unwrap();
    let got = String::from_utf8_lossy(&got).to_ascii_lowercase();
    assert!(got.starts_with("http/1.1 101"), "{host}: {got}");
    assert!(
        got.contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
        "{got}"
    );
    assert!(got.ends_with("\r\n\r\ntunnel:early"), "{got}");
    let seen = s.tunneled.recv().await.unwrap();
    assert!(seen.upgraded);
    assert_eq!(seen.path, "/burrow");
    assert_eq!(seen.local, http);
}

fn get(host: &str, target: &str) -> String {
    format!("GET {target} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
}

#[tokio::test]
async fn the_tunnel_on_its_own_name_takes_a_plain_ws_upgrade_on_port_80_beside_sites() {
    let mut s = on_port_80("tunnel.example.com", TunnelMode::OwnName).await;
    upgraded(&mut s, "tunnel.example.com").await;
    upgraded(&mut s, "Tunnel.Example.com:80").await;
    let http = s.proxy.http_addr().unwrap();
    let r = over_plain(http, &get("tunnel.example.com", "/burrow")).await;
    assert!(r.starts_with("HTTP/1.1 426"), "{r}");
    let r = over_plain(http, &get("tunnel.example.com", "/other")).await;
    assert!(r.starts_with("HTTP/1.1 404"), "{r}");
    let r = over_plain(http, &get("wiki.example.com", "/burrow")).await;
    assert!(
        r.starts_with("HTTP/1.1 200"),
        "a plain site keeps its paths: {r}"
    );
    let r = over_plain(http, &get("shop.example.com", "/burrow")).await;
    assert!(r.starts_with("HTTP/1.1 308"), "{r}");
    assert!(
        closed_without_tls(s.proxy.https_addr().unwrap(), "tunnel.example.com").await,
        "the tunnel rides only the port it shares"
    );
}

#[tokio::test]
async fn the_tunnel_by_path_on_a_redirected_site_is_never_redirected() {
    let mut s = on_port_80("shop.example.com", TunnelMode::ByPath).await;
    upgraded(&mut s, "shop.example.com").await;
    let http = s.proxy.http_addr().unwrap();
    let r = over_plain(http, &get("shop.example.com", "/burrow")).await;
    assert!(r.starts_with("HTTP/1.1 426"), "{r}");
    let r = over_plain(http, &get("shop.example.com", "/cart")).await;
    assert!(
        r.starts_with("HTTP/1.1 308"),
        "the site keeps the rest: {r}"
    );
    s.certs
        .tokens
        .lock()
        .unwrap()
        .insert("tok_EN-1".into(), "tok_EN-1.thumb".into());
    let r = over_plain(
        http,
        &get("shop.example.com", "/.well-known/acme-challenge/tok_EN-1"),
    )
    .await;
    assert!(r.ends_with("\r\n\r\ntok_EN-1.thumb"), "{r}");
    let r = over_tls(
        s.proxy.https_addr().unwrap(),
        "shop.example.com",
        &get("shop.example.com", "/burrow"),
    )
    .await;
    assert!(
        r.starts_with("HTTP/1.1 200"),
        "over HTTPS the path is the site's: {r}"
    );
}

#[tokio::test]
async fn the_tunnel_by_path_on_a_plain_site_leaves_it_the_other_paths() {
    let mut s = on_port_80("wiki.example.com", TunnelMode::ByPath).await;
    upgraded(&mut s, "wiki.example.com").await;
    let http = s.proxy.http_addr().unwrap();
    let r = over_plain(http, &get("wiki.example.com", "/burrow/x")).await;
    assert!(r.starts_with("HTTP/1.1 426"), "{r}");
    let r = over_plain(http, &get("wiki.example.com", "/burrowed")).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
}
