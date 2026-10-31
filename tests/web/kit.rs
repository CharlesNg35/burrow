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

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn acceptor(c: &Cert) -> TlsAcceptor {
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
    trusting_alpn(c, &[])
}

pub fn trusting_alpn(c: &Cert, alpn: &[&[u8]]) -> TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(c.cert_pem.as_bytes()).unwrap())
        .unwrap();
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
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

pub async fn visit(
    addr: SocketAddr,
    name: &str,
    c: &Cert,
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    visit_over(TcpStream::connect(addr).await?, name, c).await
}

pub async fn visit_with_small_send_buffer(
    addr: SocketAddr,
    name: &str,
    c: &Cert,
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_send_buffer_size(4096)?;
    visit_over(socket.connect(addr).await?, name, c).await
}

async fn visit_over(
    tcp: TcpStream,
    name: &str,
    c: &Cert,
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
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
            WebSite::passthrough(hosts.iter().map(|h| h.to_string()).collect()),
        );
    }
    let mut settings = WebSettings::new("127.0.0.1:0".parse().unwrap());
    settings.sites = web;
    s.web = Some(settings);
    s
}

pub fn share_tunnel(s: &mut ServerSettings, host: &str, c: &Cert) {
    let https = s.web.as_ref().unwrap().https.unwrap();
    s.listen = None;
    let dir = tempfile::tempdir().unwrap().keep();
    let (cert_path, key_path) = (dir.join("cert.pem"), dir.join("key.pem"));
    std::fs::write(&cert_path, &c.cert_pem).unwrap();
    std::fs::write(&key_path, &c.key_pem).unwrap();
    s.websocket = Some(WebSocketSettings {
        listen: https,
        path: "/burrow".into(),
        tls: Some(TlsMaterial {
            cert_path,
            key_path,
            cert_pem: c.cert_pem.clone().into_bytes(),
            key_pem: zeroize::Zeroizing::new(c.key_pem.clone().into_bytes()),
        }),
        keepalive: Duration::from_secs(25),
        host: Some(host.into()),
        shares: burrow::config::settings::Shares::Https,
    });
    s.web.as_mut().unwrap().tunnel = Some(host.into());
}

pub async fn closed(addr: SocketAddr, name: &str) -> bool {
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(burrow_web::http::upstream::target_tls::any_certificate())
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

pub fn client_hello(name: &str) -> Vec<u8> {
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(burrow_web::http::upstream::target_tls::any_certificate())
        .with_no_client_auth();
    let name = ServerName::try_from(name.to_string()).unwrap();
    let mut conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
    let mut out = Vec::new();
    while conn.wants_write() {
        conn.write_tls(&mut out).unwrap();
    }
    out
}

pub async fn counting_backend() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut all = Vec::new();
                if s.read_to_end(&mut all).await.is_ok() {
                    let _ = s.write_all(format!("got {}", all.len()).as_bytes()).await;
                    let _ = s.shutdown().await;
                }
            });
        }
    });
    addr
}

pub async fn reset_quickly(s: &mut TcpStream) -> bool {
    let mut buf = [0u8; 256];
    loop {
        match tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf)).await {
            Ok(Ok(0)) => return false,
            Ok(Ok(_)) => continue,
            Ok(Err(e)) => return e.kind() == std::io::ErrorKind::ConnectionReset,
            Err(_) => panic!("the connection is still open after 5 s; it must be reset quickly"),
        }
    }
}

pub async fn clean_answer(s: &mut TcpStream) -> String {
    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut got))
        .await
        .expect("an answer within 5 s")
        .expect("a normal close, not a reset");
    String::from_utf8_lossy(&got).into_owned()
}

pub async fn http_app() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut one = [0u8; 1];
                loop {
                    head.clear();
                    while !head.ends_with(b"\r\n\r\n") {
                        match s.read(&mut one).await {
                            Ok(1) => head.push(one[0]),
                            _ => return,
                        }
                    }
                    let text = String::from_utf8_lossy(&head).to_ascii_lowercase();
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{text}",
                        text.len()
                    );
                    if s.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

pub fn plain_server(secret: &Secret, sites: &[(&str, Service, &[&str])]) -> ServerSettings {
    let mut s = web_server(secret, sites);
    let web = s.web.as_mut().unwrap();
    web.https = None;
    web.http = Some(
        format!("127.0.0.1:{}", crate::common::free_port())
            .parse()
            .unwrap(),
    );
    for site in web.sites.values_mut() {
        site.tls = burrow::config::settings::WebTls::Off;
        site.https_redirect = false;
    }
    s
}

pub async fn plain_get(addr: SocketAddr, host: &str, path: &str) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    clean_answer(&mut s).await
}
