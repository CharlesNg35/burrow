use std::sync::Arc;

use tokio_rustls::rustls::server::{ClientHello, ResolvesServerCert};
use tokio_rustls::rustls::sign::CertifiedKey;

use super::{CertName, CertState, CertStatus, Certificates};
use crate::CertSettings;

#[derive(Debug)]
pub struct Fixed {
    key: Arc<CertifiedKey>,
}

impl Fixed {
    pub fn new(key: Arc<CertifiedKey>) -> Fixed {
        Fixed { key }
    }
}

impl ResolvesServerCert for Fixed {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.key.clone())
    }
}

impl Certificates for Fixed {
    fn state(&self, _name: &str) -> CertState {
        CertState::Ready
    }

    fn alpn_challenge(&self, _name: &str) -> Option<Arc<CertifiedKey>> {
        None
    }

    fn http_challenge(&self, _token: &str) -> Option<String> {
        None
    }

    fn apply(&self, _settings: &CertSettings, _names: &[CertName]) {}

    fn status(&self) -> Vec<CertStatus> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
    use tokio_rustls::rustls::{self, ClientConfig, RootCertStore, ServerConfig};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    use super::*;

    fn provider() -> Arc<rustls::crypto::CryptoProvider> {
        Arc::new(rustls::crypto::ring::default_provider())
    }

    #[tokio::test]
    async fn a_fixed_certificate_serves_a_handshake_through_the_resolver() {
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(vec!["site.example.com".into()]).unwrap();
        let cert = params.self_signed(&key).unwrap();
        let der = CertificateDer::from(cert.der().to_vec());
        let private = PrivateKeyDer::try_from(key.serialize_der()).unwrap();
        let signing = provider().key_provider.load_private_key(private).unwrap();
        let fixed = Arc::new(Fixed::new(Arc::new(CertifiedKey::new(
            vec![der.clone()],
            signing,
        ))));
        assert_eq!(fixed.state("site.example.com"), CertState::Ready);
        assert!(fixed.status().is_empty() && fixed.http_challenge("t").is_none());
        let certs: Arc<dyn Certificates> = fixed;
        let server = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(certs);
        let mut roots = RootCertStore::empty();
        roots.add(der).unwrap();
        let client = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let (a, b) = tokio::io::duplex(16 * 1024);
        let accept = TlsAcceptor::from(Arc::new(server));
        let served = tokio::spawn(async move {
            let mut tls = accept.accept(b).await.unwrap();
            tls.write_all(b"ok").await.unwrap();
            tls.shutdown().await.unwrap();
        });
        let name = ServerName::try_from("site.example.com").unwrap();
        let mut tls = TlsConnector::from(Arc::new(client))
            .connect(name, a)
            .await
            .unwrap();
        let mut got = Vec::new();
        tls.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"ok");
        served.await.unwrap();
    }
}
