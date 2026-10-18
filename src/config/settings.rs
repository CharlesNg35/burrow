use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::time::Duration;

use super::model::{Cipher, Mode, Protocol};
use crate::allowlist::{AllowList, Sources};
use crate::protocol::VisitTarget;
use crate::proxy::ProxyConfig;
use crate::secret::Secret;
use crate::transport::endpoint::Endpoint;

#[derive(Debug, Clone, PartialEq)]
pub struct SocketTuning {
    pub nodelay: bool,
    pub keepalive: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerSettings {
    pub listen: Option<SocketAddr>,
    pub public_address: Option<String>,
    pub websocket: Option<WebSocketSettings>,
    pub clients: BTreeMap<String, ClientAuth>,
    pub visitors: BTreeMap<String, VisitorAuth>,
    pub services: BTreeMap<String, Service>,
    pub insecure_plaintext: bool,
    pub heartbeat_interval: Duration,
    pub handshake_timeout: Duration,
    pub max_handshakes: usize,
    pub max_connections_per_client: usize,
    pub max_pending: usize,
    pub pool_min_idle: usize,
    pub pool_max_idle: usize,
    pub tcp: SocketTuning,
    pub udp_session_timeout: Duration,
    pub metrics: Option<SocketAddr>,
}

impl ServerSettings {
    pub fn services_for(&self, client: &str) -> Vec<crate::protocol::ServiceInfo> {
        services_for(&self.services, client)
    }
}

pub fn services_for(
    services: &BTreeMap<String, Service>,
    client: &str,
) -> Vec<crate::protocol::ServiceInfo> {
    services
        .iter()
        .filter(|(_, svc)| svc.clients.iter().any(|c| c == client))
        .map(|(name, svc)| crate::protocol::ServiceInfo {
            name: name.clone(),
            protocol: svc.protocol,
            bind_port: svc.bind.map_or(0, |b| b.port()),
            target: svc.target.clone(),
            ports: svc.ports,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientAuth {
    pub secrets: Vec<Secret>,
    pub max_connections: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisitorAuth {
    pub secrets: Vec<Secret>,
    pub reach: BTreeSet<String>,
    pub max_connections: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Health {
    pub interval: Duration,
    pub timeout: Duration,
    pub failures: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Service {
    pub clients: Vec<String>,
    pub mode: Mode,
    pub health: Option<Health>,
    pub bind: Option<SocketAddr>,
    pub target: String,
    pub ports: u16,
    pub protocol: Protocol,
    pub max_connections: Option<usize>,
    pub visitors: Vec<String>,
    pub sources: Sources,
    pub proxy_protocol: Option<crate::proxy_protocol::Version>,
    pub bandwidth: Option<u64>,
}

impl Default for Service {
    fn default() -> Self {
        Service {
            clients: Vec::new(),
            mode: Mode::Failover,
            health: None,
            bind: None,
            target: String::new(),
            ports: 1,
            protocol: Protocol::Tcp,
            max_connections: None,
            visitors: Vec::new(),
            sources: Sources::default(),
            proxy_protocol: None,
            bandwidth: None,
        }
    }
}

impl Service {
    pub fn target_at(&self, index: u16) -> String {
        crate::config::check::ports::port_at(&self.target, index)
    }

    pub fn binds(&self) -> Vec<(u16, SocketAddr)> {
        self.bind
            .map(|b| {
                (0..self.ports.max(1))
                    .map(|i| (i, SocketAddr::new(b.ip(), b.port().saturating_add(i))))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn shown_target(&self) -> String {
        match self.target.rsplit_once(':') {
            Some((host, port)) if self.ports > 1 => match port.parse::<u16>() {
                Ok(p) => format!(
                    "{host}:{}",
                    crate::config::check::ports::show(p, self.ports)
                ),
                Err(_) => self.target.clone(),
            },
            _ => self.target.clone(),
        }
    }

    pub fn shown_bind(&self) -> Option<String> {
        self.bind
            .map(|b| crate::config::check::ports::show_addr(b, self.ports))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisitorSettings {
    pub server: String,
    pub endpoint: Endpoint,
    pub headers: Vec<(String, String)>,
    pub ca_pem: Option<Vec<u8>>,
    pub ca_file: Option<std::path::PathBuf>,
    pub name: String,
    pub secret: Secret,
    pub cipher: Cipher,
    pub proxy: Option<ProxyConfig>,
    pub connect_timeout: Duration,
    pub tcp: SocketTuning,
    pub forwards: BTreeMap<String, Forward>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forward {
    pub local: SocketAddr,
    pub ports: u16,
    pub to: VisitTarget,
    pub socks: bool,
}

impl Forward {
    pub fn single(local: SocketAddr, to: VisitTarget) -> Self {
        Forward {
            local,
            ports: 1,
            to,
            socks: false,
        }
    }

    pub fn socks(local: SocketAddr, client: String) -> Self {
        Forward {
            local,
            ports: 1,
            to: VisitTarget::Client {
                client,
                target: String::new(),
            },
            socks: true,
        }
    }

    pub fn listens(&self) -> Vec<(u16, SocketAddr)> {
        (0..self.ports.max(1))
            .map(|i| {
                (
                    i,
                    SocketAddr::new(self.local.ip(), self.local.port().saturating_add(i)),
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientSettings {
    pub server: String,
    pub endpoint: Endpoint,
    pub headers: Vec<(String, String)>,
    pub ca_pem: Option<Vec<u8>>,
    pub ca_file: Option<std::path::PathBuf>,
    pub name: String,
    pub secret: Secret,
    pub allow: AllowList,
    pub insecure_plaintext: bool,
    pub cipher: Cipher,
    pub proxy: Option<ProxyConfig>,
    pub reconnect_initial: Duration,
    pub reconnect_max: Duration,
    pub give_up_after: Option<Duration>,
    pub exit_on_rejected: bool,
    pub reject_confirm: Duration,
    pub connect_timeout: Duration,
    pub tcp: SocketTuning,
    pub udp_session_timeout: Duration,
}

#[derive(Clone, PartialEq)]
pub struct WebSocketSettings {
    pub listen: SocketAddr,
    pub path: String,
    pub tls: Option<TlsMaterial>,
    pub keepalive: std::time::Duration,
}

#[derive(Clone, PartialEq)]
pub struct TlsMaterial {
    pub cert_path: std::path::PathBuf,
    pub key_path: std::path::PathBuf,
    pub cert_pem: Vec<u8>,
    pub key_pem: zeroize::Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for WebSocketSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketSettings")
            .field("listen", &self.listen)
            .field("path", &self.path)
            .field(
                "tls",
                &self.tls.as_ref().map(|t| (&t.cert_path, &t.key_path)),
            )
            .finish()
    }
}
