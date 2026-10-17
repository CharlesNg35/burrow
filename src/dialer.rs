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
use crate::transport::endpoint::Endpoint;
use crate::transport::proxy::dial_tcp;
use crate::transport::resolve::{self, Resolver};
use crate::transport::tcp::tune;
use crate::transport::ws::{self, Carrier};
use crate::transport::{Conn, tls};

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
        }
    }

    pub fn is_websocket(&self) -> bool {
        self.endpoint.is_websocket()
    }

    pub async fn connect(&self) -> Result<Conn, DialError> {
        let via = self
            .proxy
            .as_ref()
            .map(|p| format!(" via proxy {p:?}"))
            .unwrap_or_default();
        let stream = dial_tcp(
            &self.endpoint.dial_address(),
            self.proxy.as_ref(),
            self.connect_timeout,
            &*self.resolver,
        )
        .await
        .map_err(|e| {
            DialError::Unreachable(format!(
                "server unreachable at {}{via} ({})",
                self.server,
                io_reason(&e)
            ))
        })?;
        tune(&stream, &self.tcp);
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
        let conn = self.connect().await?;
        let preamble = Preamble {
            kind,
            cipher: self.cipher,
            plaintext: self.plaintext,
            name: self.name.clone(),
        };
        let fut = initiate(conn, &preamble, &self.secret, &hello);
        match tokio::time::timeout(self.connect_timeout, fut).await {
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
