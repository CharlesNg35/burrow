//! The web proxy on burrow's HTTPS and HTTP ports: it routes by TLS name and Host, passes TLS through, ends TLS and
//! reverse-proxies HTTP/1.1 and HTTP/2 to services, gets certificates (files, ACME, local CA) and hands tunnel connections to burrow.
//! It must not depend on burrow, burrow-config, burrow-admin-* or burrow-ui-kit.

#![forbid(unsafe_code)]

#[cfg(all(feature = "admin-test-hooks", not(debug_assertions)))]
compile_error!("admin-test-hooks is for tests only and never goes into a release build");

pub mod auth;
pub mod http;
pub mod io;
mod listen;
pub mod log;
pub mod peek;
mod permit;
pub mod proxy_header;
pub mod route;
pub mod site;
mod terminate;
pub mod tls;
pub mod tunnel;

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;

pub use io::{BoxIo, Ending, Io, Pipe, Prefixed, pipe};
pub use listen::{Listeners, Proxy};
pub use permit::source_key;
pub use site::{
    AccessLog, BasicUser, ClientAuth, ForwardAuth, Forwarding, HeaderRule, LogFormat, NameRedirect,
    OidcProvider, Redirect, SiteLogin, SiteOptions, TargetTls,
};
pub use tls::acme::dns01::{DnsEntry, DnsProvider, Secret, TsigAlgorithm};

#[derive(Clone)]
pub struct ProxySettings {
    pub https: Option<SocketAddr>,
    pub http: Option<SocketAddr>,
    pub sites: Vec<Site>,
    pub tunnel: Option<TunnelSettings>,
    pub timeouts: Timeouts,
    pub max_pending: usize,
    pub certs: CertSettings,
    pub redirect_names: Vec<NameRedirect>,
    pub forwarding: Forwarding,
    pub access_log: Option<AccessLog>,
    pub request_id: bool,
    pub http2: bool,
    pub oidc: Vec<OidcProvider>,
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
            redirect_names: Vec::new(),
            forwarding: Forwarding::default(),
            access_log: None,
            request_id: false,
            http2: true,
            oidc: Vec::new(),
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
    pub handshake: Duration,
    pub request_header: Duration,
    pub idle: Duration,
}

impl Default for Timeouts {
    fn default() -> Timeouts {
        use burrow_syntax::defaults::{HANDSHAKE_TIMEOUT, IDLE_TIMEOUT, REQUEST_HEADER_TIMEOUT};
        Timeouts {
            handshake: HANDSHAKE_TIMEOUT,
            request_header: REQUEST_HEADER_TIMEOUT,
            idle: IDLE_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub service: String,
    pub hosts: Vec<String>,
    pub tls: SiteTls,
    pub https_redirect: bool,
    pub options: SiteOptions,
}

impl Site {
    pub fn passthrough(service: &str, hosts: Vec<String>) -> Site {
        Site {
            service: service.to_string(),
            hosts,
            tls: SiteTls::Passthrough,
            https_redirect: true,
            options: SiteOptions::default(),
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
    Internal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CertSettings {
    pub shared: Vec<SharedFile>,
    pub tunnel_file: Option<(PathBuf, PathBuf)>,
    pub acme: Option<AcmeSettings>,
    pub choices: std::collections::BTreeMap<String, burrow_syntax::web::Challenge>,
    pub state_dir: Option<PathBuf>,
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
    pub dns: Vec<DnsEntry>,
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
pub enum Admission {
    In,
    Refused,
    Region { credit: bool },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Way {
    pub id: u64,
    pub client: Arc<str>,
    pub bound: Option<(SocketAddr, SocketAddr)>,
}

pub struct Lease(Box<dyn std::any::Any + Send + Sync>);

impl Lease {
    pub fn new<T: Send + Sync + 'static>(held: T) -> Lease {
        Lease(Box::new(held))
    }

    pub fn held<T: 'static>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ask {
    Site,
    Auth,
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

    fn answered(&self, _service: &str, _status: ::http::StatusCode) {}

    fn crowded(&self, _service: &str) -> bool {
        false
    }

    fn open(
        &self,
        service: &str,
        from: &Visitor,
    ) -> impl Future<Output = Result<BoxIo, Refused>> + Send;

    fn allowed(&self, _service: &str, _from: &Visitor) -> Admission {
        Admission::In
    }

    fn lease(&self, service: &str, from: &Visitor, ask: Ask) -> Result<(Way, Lease), Refused>;

    fn open_pooled(
        &self,
        service: &str,
        from: &Visitor,
        _lease: &Lease,
    ) -> impl Future<Output = Result<BoxIo, Refused>> + Send {
        self.open(service, from)
    }

    fn keeps(&self, _service: &str, _from: &Visitor, _ask: Ask) -> bool {
        true
    }

    fn tunnel_permit(&self, peer: SocketAddr) -> Option<Self::Permit>;

    fn tunnel(&self, conn: TunnelConn<Self::Permit>) -> impl Future<Output = ()> + Send;
}
