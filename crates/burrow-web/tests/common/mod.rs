#![allow(dead_code)]

pub mod devices;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use burrow_web::{BoxIo, ProxySettings, Refused, Site, TunnelConn, Upstream, Visitor};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
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
    connector_alpn(&[b"http/1.1"])
}

pub fn connector_alpn(alpn: &[&[u8]]) -> TlsConnector {
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCert))
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
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
                    if tls.write_all(&out).await.is_err() || tls.flush().await.is_err() {
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

#[derive(Debug, Clone, Copy)]
pub enum Script {
    RefuseLater(Refused),
    Reset,
    DieMidResponse,
}

pub struct Fake {
    pub script: Mutex<Option<Script>>,
    pub backends: Mutex<HashMap<String, SocketAddr>>,
    pub visitors: Mutex<Vec<(String, Visitor)>>,
    pub refuse: Mutex<Option<Refused>>,
    pub denied: Mutex<Vec<std::net::IpAddr>>,
    pub answered: Mutex<Vec<(String, u16)>>,
    pub tunnel_slots: Arc<Semaphore>,
    pub tunneled: mpsc::UnboundedSender<Tunneled>,
}

impl Fake {
    pub fn new(slots: usize) -> (Arc<Fake>, mpsc::UnboundedReceiver<Tunneled>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let fake = Fake {
            script: Mutex::default(),
            backends: Mutex::default(),
            visitors: Mutex::default(),
            refuse: Mutex::default(),
            denied: Mutex::default(),
            answered: Mutex::default(),
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

    fn answered(&self, service: &str, status: http::StatusCode) {
        self.answered
            .lock()
            .unwrap()
            .push((service.to_string(), status.as_u16()));
    }

    async fn open(&self, service: &str, from: &Visitor) -> Result<BoxIo, Refused> {
        if let Some(r) = *self.refuse.lock().unwrap() {
            return Err(r);
        }
        if self.denied.lock().unwrap().contains(&from.addr.ip()) {
            return Err(Refused::NotAllowed);
        }
        if let Some(script) = *self.script.lock().unwrap() {
            return Ok(Box::new(scripted(script)));
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

fn scripted(script: Script) -> burrow_web::Pipe {
    let (ours, theirs) = burrow_web::pipe();
    tokio::spawn(async move {
        let mut theirs = theirs;
        let mut buf = vec![0u8; 4096];
        let _ = theirs.read(&mut buf).await;
        match script {
            Script::RefuseLater(why) => theirs.refuse(why),
            Script::Reset => {}
            Script::DieMidResponse => {
                let _ = theirs
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial")
                    .await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        theirs.set_reset(true);
    });
    ours
}

pub fn site(service: &str, hosts: &[&str]) -> Site {
    Site::passthrough(service, hosts.iter().map(|h| h.to_string()).collect())
}

pub fn fixed(names: &[&str]) -> Arc<burrow_web::tls::Fixed> {
    let key = rcgen::KeyPair::generate().unwrap();
    let params =
        rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
            .unwrap();
    let cert = params.self_signed(&key).unwrap();
    let der = CertificateDer::from(cert.der().to_vec());
    let private = PrivateKeyDer::try_from(key.serialize_der()).unwrap();
    let signing = provider().key_provider.load_private_key(private).unwrap();
    Arc::new(burrow_web::tls::Fixed::new(Arc::new(
        rustls::sign::CertifiedKey::new(vec![der], signing),
    )))
}

pub async fn start_with(
    settings: impl FnOnce(&mut ProxySettings),
    up: Arc<Fake>,
    certs: Arc<dyn burrow_web::tls::Certificates>,
    plain: bool,
) -> burrow_web::Proxy<Fake> {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = if plain {
        Some(TcpListener::bind("127.0.0.1:0").await.unwrap())
    } else {
        None
    };
    let mut s = ProxySettings::new(l.local_addr().unwrap());
    s.http = http.as_ref().map(|h| h.local_addr().unwrap());
    settings(&mut s);
    let listeners = burrow_web::Listeners {
        https: Some(l),
        http,
    };
    burrow_web::Proxy::start(listeners, s, up, certs).await
}

pub async fn start(
    settings: impl FnOnce(&mut ProxySettings),
    up: Arc<Fake>,
) -> burrow_web::Proxy<Fake> {
    start_with(settings, up, fixed(&["site.example.com"]), false).await
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

pub struct HttpBackend {
    pub addr: SocketAddr,
    pub connections: Arc<std::sync::atomic::AtomicUsize>,
}

async fn answer(
    mut req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<http_body_util::Full<bytes::Bytes>>, std::convert::Infallible> {
    use http_body_util::BodyExt;
    let mut text = format!("{} {}\n", req.method(), req.uri());
    for (k, v) in req.headers() {
        text.push_str(&format!("{k}: {}\n", v.to_str().unwrap_or("?")));
    }
    if let Some(ms) = req.uri().path().strip_prefix("/slow/") {
        tokio::time::sleep(Duration::from_millis(ms.parse().unwrap_or(0))).await;
    }
    if req.uri().path() == "/upgrade" {
        let on = hyper::upgrade::on(&mut req);
        tokio::spawn(async move {
            if let Ok(up) = on.await {
                let mut io = hyper_util::rt::TokioIo::new(up);
                let mut buf = [0u8; 1024];
                while let Ok(n) = io.read(&mut buf).await {
                    if n == 0 || io.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            }
        });
        let mut r = hyper::Response::new(http_body_util::Full::default());
        *r.status_mut() = http::StatusCode::SWITCHING_PROTOCOLS;
        r.headers_mut().insert("upgrade", "echo".parse().unwrap());
        r.headers_mut()
            .insert("connection", "upgrade".parse().unwrap());
        return Ok(r);
    }
    let body = req
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes().len())
        .unwrap_or(0);
    text.push_str(&format!("body-len: {body}\n"));
    let mut r = hyper::Response::new(http_body_util::Full::new(bytes::Bytes::from(text)));
    r.headers_mut().insert("x-from-app", "yes".parse().unwrap());
    r.headers_mut()
        .insert("keep-alive", "timeout=5".parse().unwrap());
    Ok(r)
}

pub async fn http_backend() -> HttpBackend {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = connections.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        hyper_util::rt::TokioIo::new(s),
                        hyper::service::service_fn(answer),
                    )
                    .with_upgrades()
                    .await;
            });
        }
    });
    HttpBackend { addr, connections }
}

pub fn https_site(service: &str, hosts: &[&str]) -> Site {
    Site {
        tls: burrow_web::SiteTls::Https(burrow_web::CertSource::Auto),
        ..site(service, hosts)
    }
}

pub fn plain_site(service: &str, hosts: &[&str]) -> Site {
    Site {
        tls: burrow_web::SiteTls::Plain,
        https_redirect: false,
        ..site(service, hosts)
    }
}

pub async fn exchange<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    s: &mut S,
    req: &[u8],
) -> String {
    s.write_all(req).await.unwrap();
    s.flush().await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

pub async fn over_tls(addr: SocketAddr, name: &str, req: &str) -> String {
    let mut t = tls(addr, name).await.unwrap();
    exchange(&mut t, req.as_bytes()).await
}

pub async fn over_plain(addr: SocketAddr, req: &str) -> String {
    let mut t = TcpStream::connect(addr).await.unwrap();
    exchange(&mut t, req.as_bytes()).await
}

#[derive(Debug)]
pub struct Scripted {
    pub fixed: Arc<burrow_web::tls::Fixed>,
    pub state: Mutex<HashMap<String, burrow_web::tls::CertState>>,
    pub tokens: Mutex<HashMap<String, String>>,
    pub alpn: Mutex<HashMap<String, Arc<rustls::sign::CertifiedKey>>>,
}

impl Scripted {
    pub fn new() -> Arc<Scripted> {
        Arc::new(Scripted {
            fixed: fixed(&["site.example.com"]),
            state: Mutex::default(),
            tokens: Mutex::default(),
            alpn: Mutex::default(),
        })
    }
}

impl rustls::server::ResolvesServerCert for Scripted {
    fn resolve(
        &self,
        hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.fixed.resolve(hello)
    }
}

impl burrow_web::tls::Certificates for Scripted {
    fn state(&self, name: &str) -> burrow_web::tls::CertState {
        self.state
            .lock()
            .unwrap()
            .get(name)
            .copied()
            .unwrap_or(burrow_web::tls::CertState::Ready)
    }

    fn certificate(&self, name: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        burrow_web::tls::Certificates::certificate(self.fixed.as_ref(), name)
    }

    fn alpn_challenge(&self, name: &str) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.alpn.lock().unwrap().get(name).cloned()
    }

    fn http_challenge(&self, token: &str) -> Option<String> {
        self.tokens.lock().unwrap().get(token).cloned()
    }

    fn apply<'a>(
        &'a self,
        _: &'a burrow_web::CertSettings,
        _: &'a [burrow_web::tls::CertName],
    ) -> burrow_web::tls::Applying<'a> {
        Box::pin(async {})
    }

    fn status(&self) -> Vec<burrow_web::tls::CertStatus> {
        Vec::new()
    }
}

pub async fn read_response<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    s: &mut S,
    req: &str,
) -> String {
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), s.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        out.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&out).into_owned();
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text[..end]
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if out.len() >= end + 4 + len {
                return text;
            }
        }
        if n == 0 {
            return text;
        }
    }
}

pub struct Ca {
    pub params: CertificateParams,
    pub key: KeyPair,
    pub pem: String,
}

pub fn ca() -> Ca {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let pem = params.self_signed(&key).unwrap().pem();
    Ca { params, key, pem }
}

pub fn leaf(ca: Option<&Ca>, names: &[&str]) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key = KeyPair::generate().unwrap();
    let params =
        CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
    let cert = match ca {
        Some(ca) => {
            let issuer = Issuer::new(ca.params.clone(), &ca.key);
            params.signed_by(&key, &issuer).unwrap()
        }
        None => params.self_signed(&key).unwrap(),
    };
    let der = CertificateDer::from(cert.der().to_vec());
    (der, PrivateKeyDer::try_from(key.serialize_der()).unwrap())
}
