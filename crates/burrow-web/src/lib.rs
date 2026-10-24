//! The web proxy on burrow's HTTPS port: it reads the TLS name, passes TLS through to a
//! service or hands the connection to burrow's tunnel. It knows nothing about the config file.
//! It must not depend on burrow, burrow-config, burrow-admin-* or burrow-ui-kit.

#![forbid(unsafe_code)]

pub mod http;
pub mod io;
mod listen;
pub mod peek;
mod permit;
pub mod proxy_header;
pub mod route;
mod terminate;
pub mod tls;
pub mod tunnel;

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use tokio::net::TcpStream;

pub use io::{BoxIo, Ending, Io, Pipe, Prefixed, pipe};
pub use listen::{Listeners, Proxy};
pub use permit::source_key;

pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct ProxySettings {
    pub https: Option<SocketAddr>,
    pub http: Option<SocketAddr>,
    pub sites: Vec<Site>,
    pub tunnel: Option<TunnelSettings>,
    pub timeouts: Timeouts,
    pub max_pending: usize,
    pub certs: CertSettings,
}

impl Default for ProxySettings {
    fn default() -> ProxySettings {
        ProxySettings {
            https: None,
            http: None,
            sites: Vec::new(),
            tunnel: None,
            timeouts: Timeouts::default(),
            max_pending: 256,
            certs: CertSettings::default(),
        }
    }
}

impl ProxySettings {
    pub fn new(https: SocketAddr) -> ProxySettings {
        ProxySettings {
            https: Some(https),
            ..ProxySettings::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub hello: Duration,
    pub handshake: Duration,
    pub request_header: Duration,
    pub idle: Duration,
    pub response_header: Duration,
}

impl Default for Timeouts {
    fn default() -> Timeouts {
        Timeouts {
            hello: HELLO_TIMEOUT,
            handshake: Duration::from_secs(10),
            request_header: Duration::from_secs(10),
            idle: Duration::from_secs(60),
            response_header: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub service: String,
    pub hosts: Vec<String>,
    pub tls: SiteTls,
    pub https_redirect: bool,
}

impl Site {
    pub fn passthrough(service: &str, hosts: Vec<String>) -> Site {
        Site {
            service: service.to_string(),
            hosts,
            tls: SiteTls::Passthrough,
            https_redirect: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteTls {
    Passthrough,
    Https(CertSource),
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertSource {
    OwnFile { cert: PathBuf, key: PathBuf },
    Auto,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CertSettings {
    pub shared: Vec<SharedFile>,
    pub tunnel_file: Option<(PathBuf, PathBuf)>,
    pub acme: Option<AcmeSettings>,
    pub choices: std::collections::BTreeMap<String, burrow_syntax::web::Challenge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedFile {
    pub hosts: Vec<String>,
    pub cert: PathBuf,
    pub key: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeSettings {
    pub directory_url: String,
    pub ca: Option<PathBuf>,
    pub email: String,
    pub profile: Option<String>,
    pub state_dir: Option<PathBuf>,
    pub check_ports: burrow_syntax::web::CheckPorts,
    pub dns: bool,
    pub fallback_directory: Option<String>,
}

#[derive(Clone)]
pub struct TunnelSettings {
    pub host: String,
    pub path: String,
    pub mode: TunnelMode,
    pub port: TunnelPort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelPort {
    Https,
    Http,
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
    Down,
    Denied,
    Unreachable,
}

impl Refused {
    const ALL: [Refused; 7] = [
        Refused::NotAllowed,
        Refused::Full,
        Refused::NoClient,
        Refused::Gone,
        Refused::Down,
        Refused::Denied,
        Refused::Unreachable,
    ];

    fn code(self) -> u8 {
        Refused::ALL
            .iter()
            .position(|r| *r == self)
            .map_or(0, |i| u8::try_from(i + 1).unwrap_or(0))
    }

    fn from_code(code: u8) -> Option<Refused> {
        Refused::ALL.get(usize::from(code).checked_sub(1)?).copied()
    }
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

    fn refused(&self) {}

    fn open(
        &self,
        service: &str,
        from: &Visitor,
    ) -> impl Future<Output = Result<BoxIo, Refused>> + Send;

    fn tunnel_permit(&self, peer: SocketAddr) -> Option<Self::Permit>;

    fn tunnel(&self, conn: TunnelConn<Self::Permit>) -> impl Future<Output = ()> + Send;
}
