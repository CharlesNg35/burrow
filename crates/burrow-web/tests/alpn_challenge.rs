mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;

const ACME: &[u8] = b"acme-tls/1";

fn challenge_key(name: &str) -> Arc<tokio_rustls::rustls::sign::CertifiedKey> {
    let f = fixed(&[name]);
    burrow_web::tls::Certificates::certificate(f.as_ref(), name).unwrap()
}

async fn hello(
    addr: std::net::SocketAddr,
    name: &str,
    alpn: &[&[u8]],
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::try_from(name.to_string()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        connector_alpn(alpn).connect(name, tcp),
    )
    .await
    .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
}

async fn setup() -> (burrow_web::Proxy<Fake>, Arc<Scripted>, Arc<Fake>) {
    let (nas, _n) = backend("nas", &["nas.example.com"]).await;
    let (up, _) = Fake::new(4);
    up.serve("nas", nas);
    let certs = Scripted::new();
    let proxy = start_with(
        |s| {
            s.sites = vec![
                https_site("blog", &["blog.example.com"]),
                site("nas", &["nas.example.com"]),
            ];
        },
        up.clone(),
        certs.clone(),
        false,
    )
    .await;
    (proxy, certs, up)
}

#[tokio::test]
async fn a_hello_offering_only_acme_tls_gets_the_challenge_certificate_for_its_name() {
    let (proxy, certs, up) = setup().await;
    let addr = proxy.https_addr().unwrap();
    for name in ["blog.example.com", "10.113.0.203.in-addr.arpa"] {
        let key = challenge_key(name);
        certs.alpn.lock().unwrap().insert(name.into(), key.clone());
        let t = hello(addr, name, &[ACME]).await.unwrap();
        let (_, conn) = t.get_ref();
        assert_eq!(conn.alpn_protocol(), Some(ACME), "{name}");
        assert_eq!(conn.peer_certificates().unwrap()[0], key.cert[0], "{name}");
    }
    assert!(up.visitors.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_shared_config_never_negotiates_acme_tls() {
    let (proxy, _certs, _up) = setup().await;
    let addr = proxy.https_addr().unwrap();
    assert!(hello(addr, "blog.example.com", &[ACME]).await.is_err());
    let t = hello(addr, "blog.example.com", &[ACME, b"http/1.1"])
        .await
        .unwrap();
    assert_eq!(t.get_ref().1.alpn_protocol(), Some(&b"http/1.1"[..]));
    assert!(
        hello(addr, "blog.example.com", &[b"h2"]).await.is_err(),
        "h2 is not offered yet"
    );
}

#[tokio::test]
async fn a_passthrough_name_without_a_challenge_still_reaches_its_backend_with_acme_tls() {
    let (proxy, _certs, up) = setup().await;
    let _ = hello(proxy.https_addr().unwrap(), "nas.example.com", &[ACME]).await;
    let seen = up.visitors.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "nas");
}
