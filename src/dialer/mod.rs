pub mod admin;
pub(crate) mod fetch;

use std::sync::Arc;
use std::time::Duration;

use tokio_rustls::TlsConnector;

use crate::base::failure::io_reason;
use crate::config::settings::{ClientSettings, VisitorSettings};
use crate::wire::handshake::{ClientHandshakeError, initiate};
use crate::wire::protocol::{ChannelKind, Hello, Preamble};
use crate::wire::proxy::ProxyConfig;
use crate::wire::secret::Secret;
use crate::wire::transport::proxy::dial_tcp_ordered;
use crate::wire::transport::resolve::{self, Known, Recording, Resolver};
use crate::wire::transport::tcp::{SocketTuning, tune};
use crate::wire::transport::ws::{self, Carrier};
use crate::wire::transport::{Channel, Conn, tls};
use burrow_config::Cipher;
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
    reached: std::sync::Mutex<Option<Vec<std::net::SocketAddr>>>,
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

macro_rules! dialer_from {
    ($s:expr, $plaintext:expr, $role:expr) => {
        Dialer {
            server: $s.server.clone(),
            name: $s.name.clone(),
            resolver: resolve::system(),
            tls: connector(&$s.endpoint, $s.ca_pem.as_deref()),
            endpoint: $s.endpoint.clone(),
            headers: $s.headers.clone(),
            proxy: $s.proxy.clone(),
            connect_timeout: $s.connect_timeout,
            tcp: $s.tcp.clone(),
            cipher: $s.cipher,
            secret: $s.secret.clone(),
            plaintext: $plaintext,
            role: $role,
            recent: Default::default(),
            reached: Default::default(),
        }
    };
}

impl Dialer {
    pub fn client(s: &ClientSettings) -> Dialer {
        dialer_from!(s, s.insecure_plaintext, "client")
    }

    pub fn visitor(s: &VisitorSettings) -> Dialer {
        dialer_from!(s, false, "visitor")
    }

    pub fn is_websocket(&self) -> bool {
        self.endpoint.is_websocket()
    }

    pub async fn connect(&self) -> Result<Conn, DialError> {
        self.connect_from(ChannelKind::Control)
            .await
            .map(|(conn, _)| conn)
    }

    async fn dial_via(&self, resolver: &dyn Resolver) -> std::io::Result<tokio::net::TcpStream> {
        let order = |addrs: &mut Vec<std::net::SocketAddr>| {
            self.recent.lock().expect("recent lock").order(addrs);
        };
        dial_tcp_ordered(
            &self.endpoint.dial_address(),
            self.proxy.as_ref(),
            self.connect_timeout,
            resolver,
            &order,
        )
        .await
    }

    async fn reach(&self, kind: ChannelKind) -> std::io::Result<tokio::net::TcpStream> {
        let reached = self.reached.lock().expect("reached lock").clone();
        if let (ChannelKind::Data, Some(addrs)) = (kind, reached) {
            match self.dial_via(&Known(addrs)).await {
                Ok(stream) => return Ok(stream),
                Err(e) => tracing::debug!(
                    "{} did not answer at its last address ({}); looking the name up again",
                    self.server,
                    io_reason(&e)
                ),
            }
        }
        let lookup = Recording::new(&*self.resolver);
        let stream = self.dial_via(&lookup).await?;
        *self.reached.lock().expect("reached lock") = lookup.answer();
        Ok(stream)
    }

    async fn connect_from(
        &self,
        kind: ChannelKind,
    ) -> Result<(Conn, Option<std::net::SocketAddr>), DialError> {
        let via = self
            .proxy
            .as_ref()
            .map(|p| format!(" via proxy {p:?}"))
            .unwrap_or_default();
        let stream = self.reach(kind).await.map_err(|e| {
            let why = unreachable_reason(&self.server, &via, &e);
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
        let (conn, peer) = self.connect_from(kind).await?;
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

fn unreachable_reason(server: &str, via: &str, e: &std::io::Error) -> String {
    if crate::wire::transport::proxy::lookup_timed_out(e) {
        io_reason(e)
    } else {
        format!("server unreachable at {server}{via} ({})", io_reason(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lookup_that_timed_out_is_named_as_dns() {
        let lookup = std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            crate::wire::transport::proxy::LookupTimedOut::new(
                "tunnel.example.com:7835",
                std::time::Duration::from_secs(10),
            ),
        );
        assert_eq!(
            unreachable_reason("tunnel.example.com:7835", "", &lookup),
            "could not look up tunnel.example.com within 10 s (DNS)"
        );
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert_eq!(
            unreachable_reason("tunnel.example.com:7835", "", &refused),
            "server unreachable at tunnel.example.com:7835 (connection refused)"
        );
        let slow = std::io::Error::from(std::io::ErrorKind::TimedOut);
        assert_eq!(
            unreachable_reason("tunnel.example.com:7835", "", &slow),
            "server unreachable at tunnel.example.com:7835 (timed out)"
        );
    }

    struct Dns(std::sync::Mutex<Option<std::net::SocketAddr>>);

    impl Resolver for Dns {
        fn resolve<'a>(&'a self, _: &'a str) -> resolve::Resolved<'a> {
            let answer = *self.0.lock().unwrap();
            Box::pin(std::future::ready(answer.map(|a| vec![a]).ok_or_else(
                || std::io::Error::other("failed to lookup address information: Try again"),
            )))
        }
    }

    struct Plain {
        server: String,
        name: String,
        endpoint: Endpoint,
        headers: Vec<(String, String)>,
        proxy: Option<ProxyConfig>,
        connect_timeout: Duration,
        tcp: SocketTuning,
        cipher: Cipher,
        secret: Secret,
        ca_pem: Option<Vec<u8>>,
    }

    fn plain_dialer(dns: Arc<Dns>) -> Dialer {
        let s = Plain {
            server: "tunnel.example.test:7835".into(),
            name: "home".into(),
            endpoint: Endpoint::parse("tunnel.example.test:7835").unwrap(),
            headers: Vec::new(),
            proxy: None,
            connect_timeout: Duration::from_secs(2),
            tcp: SocketTuning {
                nodelay: true,
                keepalive: None,
            },
            cipher: Cipher::Chachapoly,
            secret: Secret::generate(),
            ca_pem: None,
        };
        let mut d = dialer_from!(s, false, "client");
        d.resolver = dns;
        d
    }

    async fn listen() -> (tokio::net::TcpListener, std::net::SocketAddr) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = l.local_addr().unwrap();
        (l, a)
    }

    #[tokio::test]
    async fn data_channels_reuse_the_reached_address_and_look_up_again_when_it_fails() {
        let (first, at_first) = listen().await;
        let dns = Arc::new(Dns(std::sync::Mutex::new(Some(at_first))));
        let d = plain_dialer(dns.clone());
        let peer = |s: std::io::Result<tokio::net::TcpStream>| s.unwrap().peer_addr().unwrap();
        assert_eq!(peer(d.reach(ChannelKind::Control).await), at_first);
        *dns.0.lock().unwrap() = None;
        assert_eq!(peer(d.reach(ChannelKind::Data).await), at_first);
        drop(first);
        let (_second, at_second) = listen().await;
        *dns.0.lock().unwrap() = Some(at_second);
        assert_eq!(
            peer(d.reach(ChannelKind::Data).await),
            at_second,
            "a remembered address that fails is looked up again"
        );
        *dns.0.lock().unwrap() = None;
        assert_eq!(peer(d.reach(ChannelKind::Data).await), at_second);
        assert!(
            d.reach(ChannelKind::Control).await.is_err(),
            "a new control connection always looks the name up"
        );
        let (_third, at_third) = listen().await;
        *dns.0.lock().unwrap() = Some(at_third);
        assert_eq!(peer(d.reach(ChannelKind::Control).await), at_third);
        assert_eq!(peer(d.reach(ChannelKind::Data).await), at_third);
        assert_eq!(peer(d.reach(ChannelKind::Visit).await), at_third);
    }

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
