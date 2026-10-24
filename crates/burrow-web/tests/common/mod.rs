#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow_web::{BoxIo, ProxySettings, Refused, Site, TunnelConn, Upstream, Visitor};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio_rustls::rustls::{self, ClientConfig, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

pub fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn acceptor(names: &[&str]) -> TlsAcceptor {
    let key = rcgen::KeyPair::generate().unwrap();
    let params =
        rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
            .unwrap();
    let cert = params.self_signed(&key).unwrap();
    let certs = vec![CertificateDer::from_pem_slice(cert.pem().as_bytes()).unwrap()];
    let key = PrivateKeyDer::from_pem_slice(key.serialize_pem().as_bytes()).unwrap();
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    TlsAcceptor::from(Arc::new(config))
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

pub fn connector() -> TlsConnector {
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCert))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    TlsConnector::from(Arc::new(config))
}

pub async fn tls(
    addr: SocketAddr,
    name: &str,
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::try_from(name.to_string()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), connector().connect(name, tcp))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
}

pub async fn backend(
    label: &'static str,
    names: &[&str],
) -> (SocketAddr, mpsc::UnboundedReceiver<Option<String>>) {
    let acceptor = acceptor(names);
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            let (acceptor, tx) = (acceptor.clone(), tx.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(s).await else {
                    return;
                };
                let _ = tx.send(tls.get_ref().1.server_name().map(str::to_string));
                let mut buf = [0u8; 1024];
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

pub struct Tunneled {
    pub upgraded: bool,
    pub path: String,
    pub peer: SocketAddr,
    pub local: SocketAddr,
}

pub struct Fake {
    pub backends: Mutex<HashMap<String, SocketAddr>>,
    pub visitors: Mutex<Vec<(String, Visitor)>>,
    pub refuse: Mutex<Option<Refused>>,
    pub tunnel_slots: Arc<Semaphore>,
    pub tunneled: mpsc::UnboundedSender<Tunneled>,
}

impl Fake {
    pub fn new(slots: usize) -> (Arc<Fake>, mpsc::UnboundedReceiver<Tunneled>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let fake = Fake {
            backends: Mutex::default(),
            visitors: Mutex::default(),
            refuse: Mutex::default(),
            tunnel_slots: Arc::new(Semaphore::new(slots)),
            tunneled: tx,
        };
        (Arc::new(fake), rx)
    }

    pub fn serve(&self, service: &str, addr: SocketAddr) {
        self.backends
            .lock()
            .unwrap()
            .insert(service.to_string(), addr);
    }
}

impl Upstream for Fake {
    type Permit = OwnedSemaphorePermit;

    async fn open(&self, service: &str, from: &Visitor) -> Result<BoxIo, Refused> {
        if let Some(r) = *self.refuse.lock().unwrap() {
            return Err(r);
        }
        self.visitors
            .lock()
            .unwrap()
            .push((service.to_string(), from.clone()));
        let addr = self.backends.lock().unwrap().get(service).copied();
        let addr = addr.ok_or(Refused::NoClient)?;
        let s = TcpStream::connect(addr).await.map_err(|_| Refused::Gone)?;
        Ok(Box::new(s))
    }

    fn tunnel_permit(&self, _peer: SocketAddr) -> Option<Self::Permit> {
        self.tunnel_slots.clone().try_acquire_owned().ok()
    }

    async fn tunnel(&self, conn: TunnelConn<Self::Permit>) {
        let TunnelConn {
            mut io,
            upgraded,
            path,
            peer,
            local,
            permit,
            ..
        } = conn;
        let _ = self.tunneled.send(Tunneled {
            upgraded,
            path,
            peer,
            local,
        });
        let mut buf = [0u8; 1024];
        let Ok(n) = io.read(&mut buf).await else {
            return;
        };
        drop(permit);
        let mut out = b"tunnel:".to_vec();
        out.extend_from_slice(&buf[..n]);
        let _ = io.write_all(&out).await;
        let _ = io.shutdown().await;
    }
}

pub fn site(service: &str, hosts: &[&str]) -> Site {
    Site {
        service: service.into(),
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
    }
}

pub async fn start(
    settings: impl FnOnce(&mut ProxySettings),
    up: Arc<Fake>,
) -> burrow_web::Proxy<Fake> {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut s = ProxySettings::new(l.local_addr().unwrap());
    settings(&mut s);
    burrow_web::Proxy::start(l, s, up)
}

pub async fn ask<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    s: &mut S,
    what: &[u8],
) -> std::io::Result<Vec<u8>> {
    s.write_all(what).await?;
    let mut buf = vec![0u8; 2048];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
    buf.truncate(n);
    Ok(buf)
}

pub async fn closed_without_tls(addr: SocketAddr, name: &str) -> bool {
    let tcp = TcpStream::connect(addr).await.expect("the port accepts");
    let name = ServerName::try_from(name.to_string()).unwrap();
    match tokio::time::timeout(Duration::from_secs(5), connector().connect(name, tcp)).await {
        Ok(Ok(_)) => false,
        Ok(Err(_)) => true,
        Err(_) => panic!("{addr}: the connection is still open after 5 s; it must close quickly"),
    }
}
