use std::sync::Arc;

use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{
    CryptoProvider, WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature,
};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{
    ClientConfig, DigitallySignedStruct, Error as TlsError, RootCertStore, SignatureScheme,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    PublicRoots,
    OwnCa(Vec<u8>),
    Unchecked,
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(tokio_rustls::rustls::crypto::ring::default_provider())
}

pub fn ca_roots(pem: &[u8]) -> Result<RootCertStore, String> {
    let certs = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("is not a PEM certificate file ({e})"))?;
    if certs.is_empty() {
        return Err("holds no certificate".into());
    }
    let mut roots = RootCertStore::empty();
    for c in certs {
        roots.add(c).map_err(|e| e.to_string())?;
    }
    Ok(roots)
}

#[derive(Debug)]
struct AnyCertificate(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

pub fn any_certificate() -> Arc<dyn ServerCertVerifier> {
    Arc::new(AnyCertificate(provider().signature_verification_algorithms))
}

pub fn connector(check: &Check, http2: bool) -> Result<TlsConnector, String> {
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let mut config = match check {
        Check::PublicRoots => builder
            .with_root_certificates(RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })
            .with_no_client_auth(),
        Check::OwnCa(pem) => builder
            .with_root_certificates(ca_roots(pem)?)
            .with_no_client_auth(),
        Check::Unchecked => builder
            .dangerous()
            .with_custom_certificate_verifier(any_certificate())
            .with_no_client_auth(),
    };
    config.alpn_protocols = if http2 {
        vec![crate::terminate::H2.to_vec()]
    } else {
        vec![crate::terminate::HTTP_1_1.to_vec()]
    };
    Ok(TlsConnector::from(Arc::new(config)))
}

pub fn server_name(host: &str) -> Result<ServerName<'static>, String> {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    ServerName::try_from(bare.to_string()).map_err(|_| format!("\"{host}\" is not a host name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ip_target_is_checked_as_an_ip_and_a_name_as_a_name() {
        assert!(matches!(
            server_name("192.168.1.10").unwrap(),
            ServerName::IpAddress(_)
        ));
        assert!(matches!(
            server_name("[2001:db8::1]").unwrap(),
            ServerName::IpAddress(_)
        ));
        assert!(matches!(
            server_name("erp.lan").unwrap(),
            ServerName::DnsName(_)
        ));
        assert!(server_name("a b").is_err());
    }

    #[test]
    fn a_ca_file_must_hold_a_certificate() {
        assert_eq!(ca_roots(b"").unwrap_err(), "holds no certificate");
        assert!(
            ca_roots(b"-----BEGIN CERTIFICATE-----\nnot base64\n-----END CERTIFICATE-----\n")
                .is_err()
        );
        let cert = rcgen::generate_simple_self_signed(vec!["erp.lan".into()]).unwrap();
        assert_eq!(ca_roots(cert.cert.pem().as_bytes()).unwrap().len(), 1);
        for check in [
            Check::PublicRoots,
            Check::OwnCa(cert.cert.pem().into_bytes()),
            Check::Unchecked,
        ] {
            assert!(connector(&check, false).is_ok());
        }
    }
}
