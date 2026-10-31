mod common;

use std::sync::Arc;

use common::*;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::crypto::aws_lc_rs::kx_group;
use tokio_rustls::rustls::crypto::{CryptoProvider, SupportedKxGroup};
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, NamedGroup};
use tokio_websockets::{ClientBuilder, Message};

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

#[tokio::test]
async fn a_hello_without_alpn_still_reaches_a_terminated_site() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let proxy = start(
        |s| s.sites = vec![https_site("blog", &["blog.example.com"])],
        up,
    )
    .await;
    let tcp = tokio::net::TcpStream::connect(proxy.https_addr().unwrap())
        .await
        .unwrap();
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from("blog.example.com").unwrap();
    let mut t = connector_alpn(&[]).connect(name, tcp).await.unwrap();
    assert_eq!(t.get_ref().1.alpn_protocol(), None);
    let r = exchange(
        &mut t,
        b"GET /old HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        r.starts_with("HTTP/1.1 200") && r.contains("GET /old\n"),
        "{r}"
    );
}

fn offering(groups: &[&'static dyn SupportedKxGroup]) -> TlsConnector {
    let provider = CryptoProvider {
        kx_groups: groups.to_vec(),
        ..(*provider()).clone()
    };
    let config = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(burrow_web::http::upstream::target_tls::any_certificate())
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

#[tokio::test]
async fn visitors_get_the_hybrid_post_quantum_group_and_classical_ones_still_connect() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let proxy = start(
        |s| s.sites = vec![https_site("blog", &["blog.example.com"])],
        up,
    )
    .await;
    let cases: [(TlsConnector, NamedGroup); 4] = [
        (connector(), NamedGroup::X25519MLKEM768),
        (offering(&[kx_group::X25519]), NamedGroup::X25519),
        (offering(&[kx_group::SECP256R1]), NamedGroup::secp256r1),
        (offering(&[kx_group::SECP384R1]), NamedGroup::secp384r1),
    ];
    for (client, group) in cases {
        let tcp = TcpStream::connect(proxy.https_addr().unwrap())
            .await
            .unwrap();
        let name = ServerName::try_from("blog.example.com").unwrap();
        let mut t = client.connect(name, tcp).await.unwrap();
        let agreed = t.get_ref().1.negotiated_key_exchange_group().unwrap();
        assert_eq!(agreed.name(), group);
        let r = exchange(
            &mut t,
            b"GET / HTTP/1.1\r\nHost: blog.example.com\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 200"), "{group:?}: {r}");
    }
}
