mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use common::*;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::client::{
    ClientSessionMemoryCache, ClientSessionStore, Resumption, Tls12ClientSessionValue,
    Tls13ClientSessionValue,
};
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, HandshakeKind, NamedGroup};

#[derive(Debug)]
struct AnyName(ClientSessionMemoryCache);

fn one() -> ServerName<'static> {
    ServerName::try_from("any.invalid").unwrap()
}

impl ClientSessionStore for AnyName {
    fn set_kx_hint(&self, _: ServerName<'static>, group: NamedGroup) {
        self.0.set_kx_hint(one(), group);
    }

    fn kx_hint(&self, _: &ServerName<'_>) -> Option<NamedGroup> {
        self.0.kx_hint(&one())
    }

    fn set_tls12_session(&self, _: ServerName<'static>, _: Tls12ClientSessionValue) {}

    fn tls12_session(&self, _: &ServerName<'_>) -> Option<Tls12ClientSessionValue> {
        None
    }

    fn remove_tls12_session(&self, _: &ServerName<'static>) {}

    fn insert_tls13_ticket(&self, _: ServerName<'static>, value: Tls13ClientSessionValue) {
        self.0.insert_tls13_ticket(one(), value);
    }

    fn take_tls13_ticket(&self, _: &ServerName<'static>) -> Option<Tls13ClientSessionValue> {
        self.0.take_tls13_ticket(&one())
    }
}

fn resuming() -> TlsConnector {
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(burrow_web::http::upstream::target_tls::any_certificate())
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config.resumption = Resumption::store(Arc::new(AnyName(ClientSessionMemoryCache::new(32))));
    TlsConnector::from(Arc::new(config))
}

async fn visit(c: &TlsConnector, addr: SocketAddr, host: &str) -> Option<HandshakeKind> {
    let tcp = TcpStream::connect(addr).await.unwrap();
    let name = ServerName::try_from(host.to_string()).unwrap();
    let mut t = c.connect(name, tcp).await.unwrap();
    let get = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    let r = exchange(&mut t, get.as_bytes()).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    t.get_ref().1.handshake_kind()
}

#[tokio::test]
async fn a_visitor_resumes_its_session_on_the_same_name_but_never_on_another() {
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    up.serve("shop", app.addr);
    let proxy = start_with(
        |s| {
            s.sites = vec![
                https_site("blog", &["blog.example.com"]),
                https_site("shop", &["shop.example.com"]),
            ]
        },
        up,
        fixed(&["blog.example.com", "shop.example.com"]),
        false,
    )
    .await;
    let addr = proxy.https_addr().unwrap();
    let c = resuming();
    assert_eq!(
        visit(&c, addr, "blog.example.com").await,
        Some(HandshakeKind::Full)
    );
    let others = connector();
    for _ in 0..600 {
        visit(&others, addr, "blog.example.com").await;
    }
    assert_eq!(
        visit(&c, addr, "blog.example.com").await,
        Some(HandshakeKind::Resumed),
        "a session ticket from this name resumes, also after more visitors than a session cache holds"
    );
    assert_eq!(
        visit(&c, addr, "shop.example.com").await,
        Some(HandshakeKind::Full),
        "a ticket from another name gives a full handshake"
    );
}
