//! The web proxy on burrow's HTTPS port: it reads the TLS name, passes TLS through to a
//! service or hands the connection to burrow's tunnel. It knows nothing about the config file.
//! It must not depend on burrow, burrow-config, burrow-admin-* or burrow-ui-kit.

#![forbid(unsafe_code)]

pub mod io;
mod listen;
pub mod peek;
mod permit;
pub mod proxy_header;
pub mod route;
pub mod tunnel;

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;

pub use io::{BoxIo, Io, Pipe, Prefixed, pipe};
pub use listen::Proxy;
pub use permit::source_key;

pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct ProxySettings {
    pub https: SocketAddr,
    pub sites: Vec<Site>,
    pub tunnel: Option<TunnelSettings>,
    pub max_pending: usize,
    pub hello_timeout: Duration,
    pub handshake_timeout: Duration,
}

impl ProxySettings {
    pub fn new(https: SocketAddr) -> ProxySettings {
        ProxySettings {
            https,
            sites: Vec::new(),
            tunnel: None,
            max_pending: 256,
            hello_timeout: HELLO_TIMEOUT,
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub service: String,
    pub hosts: Vec<String>,
}

#[derive(Clone)]
pub struct TunnelSettings {
    pub host: String,
    pub path: String,
    pub mode: TunnelMode,
    pub tls: Option<tokio_rustls::TlsAcceptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelMode {
    OwnName,
    ByPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visitor {
    pub addr: SocketAddr,
    pub local: SocketAddr,
    pub host: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    NotAllowed,
    Full,
    NoClient,
    Gone,
}

pub struct TunnelConn<P> {
    pub io: BoxIo,
    pub upgraded: bool,
    pub path: String,
    pub peer: SocketAddr,
    pub local: SocketAddr,
    pub deadline: tokio::time::Instant,
    pub permit: P,
}

pub trait Upstream: Send + Sync + 'static {
    type Permit: Send + 'static;

    fn tune(&self, _stream: &TcpStream) {}

    fn open(
        &self,
        service: &str,
        from: &Visitor,
    ) -> impl Future<Output = Result<BoxIo, Refused>> + Send;

    fn tunnel_permit(&self, peer: SocketAddr) -> Option<Self::Permit>;

    fn tunnel(&self, conn: TunnelConn<Self::Permit>) -> impl Future<Output = ()> + Send;
}
