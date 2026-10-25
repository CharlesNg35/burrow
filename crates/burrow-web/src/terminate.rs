use std::sync::Arc;

use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::server::ResolvesServerCert;
use tokio_rustls::rustls::{self, ServerConfig};
use tracing::debug;

use crate::http::Proto;
use crate::io::{BoxIo, Prefixed};
use crate::tls::{Certificates, Fixed};

pub const H2: &[u8] = b"h2";
pub const HTTP_1_1: &[u8] = b"http/1.1";
pub const ACME_TLS: &[u8] = b"acme-tls/1";

pub fn protocols(http2: bool) -> &'static [&'static [u8]] {
    if http2 { &[H2, HTTP_1_1] } else { &[HTTP_1_1] }
}

fn config(resolver: Arc<dyn ResolvesServerCert>, alpn: &[&[u8]]) -> Arc<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut c = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    c.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Arc::new(c)
}

pub(crate) struct SiteTls {
    h1: TlsAcceptor,
    h2: TlsAcceptor,
}

impl SiteTls {
    pub(crate) fn new(certs: Arc<dyn Certificates>) -> SiteTls {
        let resolver: Arc<dyn ResolvesServerCert> = certs;
        SiteTls {
            h1: TlsAcceptor::from(config(resolver.clone(), protocols(false))),
            h2: TlsAcceptor::from(config(resolver, protocols(true))),
        }
    }

    fn shared(&self, http2: bool) -> &TlsAcceptor {
        if http2 { &self.h2 } else { &self.h1 }
    }
}

pub(crate) fn acceptor_for(
    site: &SiteTls,
    certs: &dyn Certificates,
    (name, sni): (&str, bool),
    http2: bool,
) -> Option<TlsAcceptor> {
    if sni {
        return Some(site.shared(http2).clone());
    }
    let key = certs.certificate(name)?;
    Some(TlsAcceptor::from(config(
        Arc::new(Fixed::new(key)),
        protocols(http2),
    )))
}

pub(crate) async fn accept(
    acceptor: TlsAcceptor,
    io: Prefixed<TcpStream>,
    deadline: Instant,
) -> Option<(BoxIo, Proto)> {
    let peer = io.get_ref().peer_addr().ok();
    match tokio::time::timeout_at(deadline, acceptor.accept(io)).await {
        Ok(Ok(tls)) => {
            let proto = if tls.get_ref().1.alpn_protocol() == Some(H2) {
                Proto::H2
            } else {
                Proto::H1
            };
            Some((Box::new(tls), proto))
        }
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
    let acceptor = TlsAcceptor::from(config(Arc::new(Fixed::new(key)), &[ACME_TLS]));
    if let Some((mut tls, _)) = accept(acceptor, Prefixed::new(hello, io), deadline).await {
        let _ =
            tokio::time::timeout_at(deadline, tokio::io::AsyncWriteExt::shutdown(&mut tls)).await;
    }
}
