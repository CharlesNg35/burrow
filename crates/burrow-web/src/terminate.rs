use std::sync::Arc;

use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::server::ResolvesServerCert;
use tokio_rustls::rustls::{self, ServerConfig};
use tracing::debug;

use crate::io::{BoxIo, Prefixed};
use crate::tls::{Certificates, Fixed};

pub const HTTP_1_1: &[u8] = b"http/1.1";
pub const ACME_TLS: &[u8] = b"acme-tls/1";

fn config(resolver: Arc<dyn ResolvesServerCert>, alpn: &[u8]) -> Arc<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut c = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    c.alpn_protocols = vec![alpn.to_vec()];
    Arc::new(c)
}

pub(crate) fn site_acceptor(certs: Arc<dyn Certificates>) -> TlsAcceptor {
    let resolver: Arc<dyn ResolvesServerCert> = certs;
    TlsAcceptor::from(config(resolver, HTTP_1_1))
}

pub(crate) fn acceptor_for(
    shared: &TlsAcceptor,
    certs: &dyn Certificates,
    name: &str,
    sni: bool,
) -> Option<TlsAcceptor> {
    if sni {
        return Some(shared.clone());
    }
    let key = certs.certificate(name)?;
    Some(TlsAcceptor::from(config(
        Arc::new(Fixed::new(key)),
        HTTP_1_1,
    )))
}

pub(crate) async fn accept(
    acceptor: TlsAcceptor,
    io: Prefixed<TcpStream>,
    deadline: Instant,
) -> Option<BoxIo> {
    let peer = io.get_ref().peer_addr().ok();
    match tokio::time::timeout_at(deadline, acceptor.accept(io)).await {
        Ok(Ok(tls)) => Some(Box::new(tls)),
        Ok(Err(e)) => {
            debug!(?peer, "web: TLS handshake failed: {e}");
            None
        }
        Err(_) => {
            debug!(?peer, "web: TLS handshake timed out");
            None
        }
    }
}

pub(crate) fn wants_challenge(alpn: &[Vec<u8>]) -> bool {
    matches!(alpn, [only] if only.as_slice() == ACME_TLS)
}

pub(crate) async fn answer_challenge(
    key: Arc<rustls::sign::CertifiedKey>,
    io: TcpStream,
    hello: Vec<u8>,
    deadline: Instant,
) {
    let acceptor = TlsAcceptor::from(config(Arc::new(Fixed::new(key)), ACME_TLS));
    if let Some(mut tls) = accept(acceptor, Prefixed::new(hello, io), deadline).await {
        let _ =
            tokio::time::timeout_at(deadline, tokio::io::AsyncWriteExt::shutdown(&mut tls)).await;
    }
}
