use std::sync::{Arc, LazyLock};

use tokio_rustls::rustls::client::WebPkiServerVerifier;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{CryptoProvider, aws_lc_rs};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use tokio_rustls::rustls::{
    self, ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig, SignatureScheme,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

static PROVIDER: LazyLock<Arc<CryptoProvider>> =
    LazyLock::new(|| Arc::new(aws_lc_rs::default_provider()));

pub fn provider() -> Arc<CryptoProvider> {
    PROVIDER.clone()
}

pub fn parse_certs(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, String> {
    let certs: Vec<_> = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("not a valid PEM certificate file ({e})"))?;
    if certs.is_empty() {
        return Err(
            "contains no PEM certificate (expected \"-----BEGIN CERTIFICATE-----\")".into(),
        );
    }
    Ok(certs)
}

pub fn parse_key(pem: &[u8]) -> Result<PrivateKeyDer<'static>, String> {
    PrivateKeyDer::from_pem_slice(pem).map_err(|_| {
        "contains no PEM private key (expected \"-----BEGIN PRIVATE KEY-----\" or an RSA/EC key)".to_string()
    })
}

pub fn acceptor(cert_pem: &[u8], key_pem: &[u8]) -> Result<TlsAcceptor, String> {
    let certs = parse_certs(cert_pem)?;
    let key = parse_key(key_pem)?;
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("certificate and key do not form a usable pair ({e})"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

pub fn connector(extra_ca_pem: Option<&[u8]>) -> Result<TlsConnector, String> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut pinned = Vec::new();
    if let Some(pem) = extra_ca_pem {
        for cert in parse_certs(pem)? {
            let _ = roots.add(cert.clone());
            pinned.push(cert);
        }
    }
    let builder = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let mut config = if pinned.is_empty() {
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        let inner = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider())
            .build()
            .map_err(|e| e.to_string())?;
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedOrWebPki { inner, pinned }))
            .with_no_client_auth()
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsConnector::from(Arc::new(config)))
}

#[derive(Debug)]
struct PinnedOrWebPki {
    inner: Arc<WebPkiServerVerifier>,
    pinned: Vec<CertificateDer<'static>>,
}

impl ServerCertVerifier for PinnedOrWebPki {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            Ok(v) => Ok(v),
            Err(_)
                if self
                    .pinned
                    .iter()
                    .any(|p| p.as_ref() == end_entity.as_ref()) =>
            {
                Ok(ServerCertVerified::assertion())
            }
            Err(e) => Err(e),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

pub fn server_name(host: &str) -> Result<ServerName<'static>, String> {
    ServerName::try_from(host.to_string())
        .map_err(|_| format!("\"{host}\" is not a valid TLS server name"))
}

#[cfg(test)]
mod tests {
    use tokio_rustls::rustls::NamedGroup;
    use tokio_rustls::rustls::crypto::SupportedKxGroup;
    use tokio_rustls::rustls::crypto::aws_lc_rs::kx_group;

    use super::*;

    const NAME: &str = "tunnel.example.com";

    fn server() -> (TlsAcceptor, String) {
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec![NAME.into()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let acceptor = acceptor(cert.pem().as_bytes(), key.serialize_pem().as_bytes()).unwrap();
        (acceptor, cert.pem())
    }

    fn only(groups: &[&'static dyn SupportedKxGroup], ca: &str) -> TlsConnector {
        let provider = CryptoProvider {
            kx_groups: groups.to_vec(),
            ..aws_lc_rs::default_provider()
        };
        let mut roots = RootCertStore::empty();
        roots.add_parsable_certificates(parse_certs(ca.as_bytes()).unwrap());
        let config = ClientConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        TlsConnector::from(Arc::new(config))
    }

    async fn agreed(client: TlsConnector, server: TlsAcceptor) -> NamedGroup {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let accepted = tokio::spawn(async move { server.accept(b).await.map(|_| ()) });
        let name = ServerName::try_from(NAME).unwrap();
        let tls = client.connect(name, a).await.unwrap();
        accepted.await.unwrap().unwrap();
        tls.get_ref()
            .1
            .negotiated_key_exchange_group()
            .unwrap()
            .name()
    }

    #[test]
    fn the_hybrid_post_quantum_group_comes_first() {
        let names: Vec<_> = provider().kx_groups.iter().map(|g| g.name()).collect();
        assert_eq!(
            names,
            [
                NamedGroup::X25519MLKEM768,
                NamedGroup::X25519,
                NamedGroup::secp256r1,
                NamedGroup::secp384r1
            ]
        );
    }

    #[tokio::test]
    async fn two_burrow_ends_agree_on_the_hybrid_post_quantum_group() {
        let (server, ca) = server();
        let client = connector(Some(ca.as_bytes())).unwrap();
        assert_eq!(agreed(client, server).await, NamedGroup::X25519MLKEM768);
    }

    #[tokio::test]
    async fn a_classical_client_still_connects() {
        let (server, ca) = server();
        let x25519 = only(&[kx_group::X25519], &ca);
        assert_eq!(agreed(x25519, server.clone()).await, NamedGroup::X25519);
        let p256 = only(&[kx_group::SECP256R1], &ca);
        assert_eq!(agreed(p256, server).await, NamedGroup::secp256r1);
    }
}
