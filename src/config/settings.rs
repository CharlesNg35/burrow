use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use crate::allowlist::{AllowList, Cidr, Sources};
use crate::protocol::VisitTarget;
use crate::proxy::ProxyConfig;
use crate::secret::Secret;
use burrow_config::{AdminLevel, Cipher, Mode, Protocol};
use burrow_syntax::endpoint::Endpoint;

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
    pub self_allow: Option<AllowList>,
}

pub struct ListenClash {
    pub path: Vec<String>,
    pub what: String,
    pub kept: SocketAddr,
}

impl ServerSettings {
    pub fn services_for(&self, client: &str) -> Vec<crate::protocol::ServiceInfo> {
        services_for(&self.services, client)
    }

    pub fn uses_self(&self) -> bool {
        let named = |c: &String| c == super::SELF;
        self.self_allow.is_some()
            || self.services.values().any(|s| s.clients.iter().any(named))
            || self.visitors.values().any(|v| v.reach.keys().any(named))
    }

    pub fn listen_clash(&self, next: &ServerSettings) -> Option<ListenClash> {
        let kept = self.listen.filter(|_| next.listen != self.listen)?;
        let conflicts = |a| crate::config::check::binds_conflict(kept, a);
        let clash = |path: &[&str], what: String| ListenClash {
            path: path.iter().map(|s| s.to_string()).collect(),
            what,
            kept,
        };
        next.services
            .iter()
            .find(|(_, s)| {
                s.protocol == Protocol::Tcp && s.binds().into_iter().any(|(_, a)| conflicts(a))
            })
            .map(|(n, _)| clash(&["services", n, "bind"], format!("service \"{n}\"")))
            .or_else(|| {
                next.websocket
                    .as_ref()
                    .filter(|w| conflicts(w.listen))
                    .map(|_| clash(&["websocket", "listen"], "websocket.listen".into()))
            })
            .or_else(|| {
                next.metrics
                    .filter(|m| conflicts(*m))
                    .map(|_| clash(&["metrics", "listen"], "metrics.listen".into()))
            })
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
    pub reach: BTreeMap<String, AllowList>,
    pub max_connections: Option<usize>,
    pub admin: Option<AdminLevel>,
    pub admin_from: Vec<Cidr>,
}

impl VisitorAuth {
    pub fn admits_admin(&self, from: std::net::IpAddr) -> bool {
        self.admin.is_some()
            && (self.admin_from.is_empty() || self.admin_from.iter().any(|c| c.contains(from)))
    }
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
    pub proxy_protocol: Option<burrow_config::ProxyVersion>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Format, Loaded, Role, load_str};

    fn settings(text: &str) -> ServerSettings {
        let secret = crate::secret::Secret::generate().encode();
        let text = text.replace("SECRET", &secret);
        let nothing = |_: &str| None;
        match load_str("s.yaml", &text, Format::Yaml, Some(Role::Server), &nothing).loaded {
            Some(Loaded::Server(s, _)) => *s,
            _ => panic!("loads: {text}"),
        }
    }

    #[test]
    fn a_new_listener_on_the_kept_listen_address_is_a_clash() {
        let base = "listen: 7835\nclients:\n  home: { secret: SECRET }\nservices:\n  web: { client: home, bind: 7840, target: 80 }\n";
        let old = settings(base);
        assert!(
            old.listen_clash(&old).is_none(),
            "an unchanged listen never clashes"
        );
        let moved = settings(&base.replace("listen: 7835", "listen: 7836"));
        assert!(old.listen_clash(&moved).is_none());
        let service = settings(
            &base
                .replace("listen: 7835", "listen: 7836")
                .replace("bind: 7840", "bind: 7835"),
        );
        let c = old.listen_clash(&service).unwrap();
        assert_eq!(c.path, ["services", "web", "bind"]);
        assert_eq!(c.what, "service \"web\"");
        assert_eq!(c.kept.port(), 7835);
        let ws =
            settings(&base.replace("listen: 7835", "listen: 7836\nwebsocket: { listen: 7835 }"));
        assert_eq!(old.listen_clash(&ws).unwrap().what, "websocket.listen");
        let metrics = settings(&base.replace(
            "listen: 7835",
            "listen: 7836\nmetrics: { listen: 0.0.0.0:7835 }",
        ));
        assert_eq!(old.listen_clash(&metrics).unwrap().what, "metrics.listen");
        let udp = settings(&base.replace("listen: 7835", "listen: 7836").replace(
            "bind: 7840, target: 80",
            "bind: 7835, target: 80, protocol: udp",
        ));
        assert!(
            old.listen_clash(&udp).is_none(),
            "a UDP port does not clash with a TCP listener"
        );
    }
}
