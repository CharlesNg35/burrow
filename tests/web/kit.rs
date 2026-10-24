use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use burrow::config::settings::{
    ServerSettings, Service, TlsMaterial, WebSettings, WebSite, WebSocketSettings,
};
use burrow::wire::secret::Secret;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio_rustls::rustls::{self, ClientConfig, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::common::*;

pub struct Cert {
    pub cert_pem: String,
    pub key_pem: String,
}

pub fn cert(names: &[&str]) -> Cert {
    let key = rcgen::KeyPair::generate().unwrap();
    let params =
        rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
            .unwrap();
    let c = params.self_signed(&key).unwrap();
    Cert {
        cert_pem: c.pem(),
        key_pem: key.serialize_pem(),
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn acceptor(c: &Cert) -> TlsAcceptor {
    let certs = vec![CertificateDer::from_pem_slice(c.cert_pem.as_bytes()).unwrap()];
    let key = PrivateKeyDer::from_pem_slice(c.key_pem.as_bytes()).unwrap();
    let config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    TlsAcceptor::from(Arc::new(config))
}

pub fn trusting(c: &Cert) -> TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(c.cert_pem.as_bytes()).unwrap())
        .unwrap();
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

pub struct Seen {
    pub sni: Option<String>,
    pub proxy: Option<Vec<u8>>,
}

pub async fn backend(
    label: &'static str,
    c: &Cert,
    proxy_header: bool,
) -> (SocketAddr, mpsc::UnboundedReceiver<Seen>) {
    let acceptor = acceptor(c);
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let (acceptor, tx) = (acceptor.clone(), tx.clone());
            tokio::spawn(async move {
                let proxy = if proxy_header {
                    let mut fixed = [0u8; 16];
                    if s.read_exact(&mut fixed).await.is_err() {
                        return;
                    }
                    let len = u16::from_be_bytes([fixed[14], fixed[15]]) as usize;
                    let mut body = vec![0u8; len];
                    if s.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    Some([fixed.to_vec(), body].concat())
                } else {
                    None
                };
                let Ok(mut tls) = acceptor.accept(s).await else {
                    return;
                };
                let sni = tls.get_ref().1.server_name().map(str::to_string);
                let _ = tx.send(Seen { sni, proxy });
                let mut buf = vec![0u8; 64 * 1024];
                while let Ok(n) = tls.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let mut out = format!("{label}:").into_bytes();
                    out.extend_from_slice(&buf[..n]);
                    if tls.write_all(&out).await.is_err() {
                        break;
                    }
                }
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, rx)
}

pub async fn visit(
    addr: SocketAddr,
    name: &str,
    c: &Cert,
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::try_from(name.to_string()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), trusting(c).connect(name, tcp))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
}

pub async fn ask<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    what: &[u8],
) -> std::io::Result<Vec<u8>> {
    s.write_all(what).await?;
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
    buf.truncate(n);
    Ok(buf)
}

pub fn site(clients: &[&str], target: SocketAddr) -> Service {
    Service {
        clients: clients.iter().map(|c| c.to_string()).collect(),
        bind: None,
        target: target.to_string(),
        ..Default::default()
    }
}

pub fn web_server(secret: &Secret, sites: &[(&str, Service, &[&str])]) -> ServerSettings {
    let mut s = server_settings(vec![secret.clone()]);
    let mut web = BTreeMap::new();
    for (name, svc, hosts) in sites {
        s.services.insert(name.to_string(), svc.clone());
        web.insert(
            name.to_string(),
            WebSite {
                hosts: hosts.iter().map(|h| h.to_string()).collect(),
            },
        );
    }
    s.web = Some(WebSettings {
        https: "127.0.0.1:0".parse().unwrap(),
        sites: web,
        tunnel: None,
    });
    s
}

pub fn share_tunnel(s: &mut ServerSettings, host: &str, c: &Cert) {
    let https = s.web.as_ref().unwrap().https;
    s.listen = None;
    s.websocket = Some(WebSocketSettings {
        listen: https,
        path: "/burrow".into(),
        tls: Some(TlsMaterial {
            cert_path: "cert.pem".into(),
            key_path: "key.pem".into(),
            cert_pem: c.cert_pem.clone().into_bytes(),
            key_pem: zeroize::Zeroizing::new(c.key_pem.clone().into_bytes()),
        }),
        keepalive: Duration::from_secs(25),
        host: Some(host.into()),
    });
    s.web.as_mut().unwrap().tunnel = Some(host.into());
}

#[derive(Debug)]
struct AnyCert;

impl rustls::client::danger::ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub async fn closed(addr: SocketAddr, name: &str) -> bool {
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCert))
        .with_no_client_auth();
    let Ok(tcp) = TcpStream::connect(addr).await else {
        return true;
    };
    let name = ServerName::try_from(name.to_string()).unwrap();
    let shake = TlsConnector::from(Arc::new(config)).connect(name, tcp);
    match tokio::time::timeout(Duration::from_secs(5), shake).await {
        Ok(Ok(_)) => false,
        Ok(Err(_)) => true,
        Err(_) => panic!("{addr}: the connection is still open after 5 s; it must close quickly"),
    }
}
