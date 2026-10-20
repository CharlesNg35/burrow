pub mod admin;

use std::sync::Arc;
use std::time::Duration;

use tokio_rustls::TlsConnector;

use crate::config::model::Cipher;
use crate::config::settings::{ClientSettings, SocketTuning, VisitorSettings};
use crate::failure::io_reason;
use crate::handshake::{Channel, ClientHandshakeError, initiate};
use crate::protocol::{ChannelKind, Hello, Preamble};
use crate::proxy::ProxyConfig;
use crate::secret::Secret;
use crate::transport::proxy::dial_tcp_ordered;
use crate::transport::resolve::{self, Resolver};
use crate::transport::tcp::tune;
use crate::transport::ws::{self, Carrier};
use crate::transport::{Conn, tls};
use burrow_syntax::endpoint::Endpoint;

#[derive(Debug)]
pub enum DialError {
    Unreachable(String),
    Lost(String),
    Rejected,
}

pub struct Dialer {
    pub server: String,
    pub name: String,
    pub resolver: Arc<dyn Resolver>,
    endpoint: Endpoint,
    headers: Vec<(String, String)>,
    proxy: Option<ProxyConfig>,
    connect_timeout: Duration,
    tcp: SocketTuning,
    cipher: Cipher,
    secret: Secret,
    plaintext: bool,
    tls: Option<TlsConnector>,
    role: &'static str,
    recent: std::sync::Mutex<Recent>,
}

#[derive(Default)]
struct Recent {
    good: Option<std::net::SocketAddr>,
    bad: Vec<(std::net::SocketAddr, std::time::Instant)>,
}

const BAD_FOR: std::time::Duration = std::time::Duration::from_secs(300);
const MAX_BAD: usize = 16;

impl Recent {
    fn order(&mut self, addrs: &mut Vec<std::net::SocketAddr>) {
        self.bad.retain(|(_, at)| at.elapsed() < BAD_FOR);
        if addrs.len() > 1 {
            for (bad, _) in &self.bad {
                if let Some(i) = addrs.iter().position(|a| a == bad) {
                    let a = addrs.remove(i);
                    addrs.push(a);
                }
            }
        }
        if let Some(good) = self.good
            && let Some(i) = addrs.iter().position(|a| *a == good)
        {
            let a = addrs.remove(i);
            addrs.insert(0, a);
        }
    }

    fn worked(&mut self, peer: std::net::SocketAddr, ok: bool) {
        self.bad.retain(|(a, _)| *a != peer);
        if ok {
            self.good = Some(peer);
            return;
        }
        if self.good == Some(peer) {
            self.good = None;
        }
        if self.bad.len() >= MAX_BAD {
            self.bad.remove(0);
        }
        self.bad.push((peer, std::time::Instant::now()));
    }
}

fn connector(endpoint: &Endpoint, ca_pem: Option<&[u8]>) -> Option<TlsConnector> {
    match endpoint {
        Endpoint::WebSocket { tls: true, .. } => tls::connector(ca_pem).ok(),
        _ => None,
    }
}

impl Dialer {
    pub fn client(s: &ClientSettings) -> Dialer {
        Dialer {
            server: s.server.clone(),
            name: s.name.clone(),
            resolver: resolve::system(),
            tls: connector(&s.endpoint, s.ca_pem.as_deref()),
            endpoint: s.endpoint.clone(),
            headers: s.headers.clone(),
            proxy: s.proxy.clone(),
            connect_timeout: s.connect_timeout,
            tcp: s.tcp.clone(),
            cipher: s.cipher,
            secret: s.secret.clone(),
            plaintext: s.insecure_plaintext,
            role: "client",
            recent: Default::default(),
        }
    }

    pub fn visitor(s: &VisitorSettings) -> Dialer {
        Dialer {
            server: s.server.clone(),
            name: s.name.clone(),
            resolver: resolve::system(),
            tls: connector(&s.endpoint, s.ca_pem.as_deref()),
            endpoint: s.endpoint.clone(),
            headers: s.headers.clone(),
            proxy: s.proxy.clone(),
            connect_timeout: s.connect_timeout,
            tcp: s.tcp.clone(),
            cipher: s.cipher,
            secret: s.secret.clone(),
            plaintext: false,
            role: "visitor",
            recent: Default::default(),
        }
    }

    pub fn is_websocket(&self) -> bool {
        self.endpoint.is_websocket()
    }

    pub async fn connect(&self) -> Result<Conn, DialError> {
        self.connect_from().await.map(|(conn, _)| conn)
    }

    async fn connect_from(&self) -> Result<(Conn, Option<std::net::SocketAddr>), DialError> {
        let via = self
            .proxy
            .as_ref()
            .map(|p| format!(" via proxy {p:?}"))
            .unwrap_or_default();
        let order = |addrs: &mut Vec<std::net::SocketAddr>| {
            self.recent.lock().expect("recent lock").order(addrs);
        };
        let stream = dial_tcp_ordered(
            &self.endpoint.dial_address(),
            self.proxy.as_ref(),
            self.connect_timeout,
            &*self.resolver,
            &order,
        )
        .await
        .map_err(|e| {
            let why = format!(
                "server unreachable at {}{via} ({})",
                self.server,
                io_reason(&e)
            );
            if e.kind() == std::io::ErrorKind::ConnectionAborted {
                DialError::Lost(why)
            } else {
                DialError::Unreachable(why)
            }
        })?;
        tune(&stream, &self.tcp);
        let peer = self
            .proxy
            .is_none()
            .then(|| stream.peer_addr().ok())
            .flatten();
        let upgraded = self.upgrade(stream).await;
        if let (Err(_), Some(p)) = (&upgraded, peer) {
            self.recent.lock().expect("recent lock").worked(p, false);
        }
        upgraded.map(|conn| (conn, peer))
    }

    async fn upgrade(&self, stream: tokio::net::TcpStream) -> Result<Conn, DialError> {
        let Endpoint::WebSocket { tls, host, .. } = &self.endpoint else {
            return Ok(Conn::from(stream));
        };
        let carrier = if *tls {
            let connector = self
                .tls
                .as_ref()
                .ok_or_else(|| DialError::Unreachable("TLS is not configured".into()))?;
            let name = tls::server_name(host).map_err(DialError::Unreachable)?;
            let fut = connector.connect(name, stream);
            match tokio::time::timeout(self.connect_timeout, fut).await {
                Ok(Ok(t)) => Carrier::ClientTls(Box::new(t)),
                Ok(Err(e)) => {
                    let why = format!(
                        "TLS handshake with {} failed ({})",
                        self.server,
                        tls_reason(&e, self.role)
                    );
                    return Err(if e.kind() == std::io::ErrorKind::InvalidData {
                        DialError::Unreachable(why)
                    } else {
                        DialError::Lost(why)
                    });
                }
                Err(_) => {
                    return Err(DialError::Lost(format!(
                        "TLS handshake with {} timed out",
                        self.server
                    )));
                }
            }
        } else {
            Carrier::Plain(stream)
        };
        match tokio::time::timeout(
            self.connect_timeout,
            ws::connect(carrier, &self.endpoint, &self.headers),
        )
        .await
        {
            Ok(Ok(c)) => Ok(Conn::from(c)),
            Ok(Err(ws::ConnectError::Status(code @ 502..=504))) => Err(DialError::Lost(format!(
                "{} answered HTTP {code}: the proxy in front of the server could not reach it, or the server is busy",
                self.server
            ))),
            Ok(Err(ws::ConnectError::Status(code))) => Err(DialError::Unreachable(format!(
                "{} answered HTTP {code} instead of a WebSocket upgrade (check the URL path, websocket.path in server.yaml, and the proxy forwarding)",
                self.server
            ))),
            Ok(Err(e)) => Err(DialError::Lost(format!(
                "WebSocket connection to {} failed: {}",
                self.server,
                e.describe()
            ))),
            Err(_) => Err(DialError::Lost(format!(
                "WebSocket upgrade with {} timed out",
                self.server
            ))),
        }
    }

    pub async fn open(&self, kind: ChannelKind, hello: Hello) -> Result<Channel, DialError> {
        let (conn, peer) = self.connect_from().await?;
        let preamble = Preamble {
            kind,
            cipher: self.cipher,
            plaintext: self.plaintext,
            name: self.name.clone(),
        };
        let fut = initiate(conn, &preamble, &self.secret, &hello);
        let done = tokio::time::timeout(self.connect_timeout, fut).await;
        if let Some(p) = peer {
            let ok = matches!(done, Ok(Ok(_)));
            self.recent.lock().expect("recent lock").worked(p, ok);
        }
        match done {
            Ok(Ok(ch)) => Ok(ch),
            Ok(Err(ClientHandshakeError::Rejected)) => Err(DialError::Rejected),
            Ok(Err(ClientHandshakeError::Io(e))) => Err(DialError::Lost(format!(
                "handshake with {} failed ({})",
                self.server,
                io_reason(&e)
            ))),
            Err(_) => Err(DialError::Lost(format!(
                "handshake with {} timed out",
                self.server
            ))),
        }
    }
}

fn tls_reason(e: &std::io::Error, role: &str) -> String {
    let text = e.to_string();
    if text.contains("UnknownIssuer") || text.contains("certificate") {
        format!("{text}; if the server uses a private CA, set \"ca\" in the {role} config")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_address_goes_last_and_a_working_one_first() {
        let a = |s: &str| s.parse::<std::net::SocketAddr>().unwrap();
        let (v6, v4, other) = (a("[fd00::1]:7835"), a("10.0.0.1:7835"), a("10.0.0.2:7835"));
        let mut recent = Recent::default();
        recent.worked(v6, false);
        let mut list = vec![v6, v4, other];
        recent.order(&mut list);
        assert_eq!(list, vec![v4, other, v6]);
        recent.worked(v4, false);
        let mut list = vec![v6, v4, other];
        recent.order(&mut list);
        assert_eq!(
            list,
            vec![other, v6, v4],
            "every failing address waits behind the rest"
        );
        recent.worked(other, true);
        let mut list = vec![v6, v4, other];
        recent.order(&mut list);
        assert_eq!(list, vec![other, v6, v4]);
        recent.worked(v6, true);
        let mut list = vec![v4, v6];
        recent.order(&mut list);
        assert_eq!(list, vec![v6, v4], "a recovered address is preferred again");
        let mut alone = vec![v4];
        Recent::default().order(&mut alone);
        assert_eq!(alone, vec![v4]);
    }
}
