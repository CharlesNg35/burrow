use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use crate::policy::allowlist::{AllowList, Cidr, Sources};
use crate::wire::protocol::VisitTarget;
use crate::wire::proxy::ProxyConfig;
use crate::wire::secret::Secret;
use crate::wire::transport::tcp::SocketTuning;
use burrow_admin_proto::msg::AdminLevel;
use burrow_config::{Cipher, Mode, Protocol};
use burrow_syntax::endpoint::Endpoint;

mod dns;
mod site;
mod web;

pub use dns::{DNS_WAIT, DnsProvider, DnsSettings, SecretText};
pub use site::{
    LogFormat, NameRedirect, PathRedirect, RESPONSE_HEADER_TIMEOUT, SiteOptions, TargetTls,
};

pub use web::{
    CertFile, LETSENCRYPT_PRODUCTION, LETSENCRYPT_STAGING, LetsEncrypt, WebSettings, WebSite,
    WebTls, digest_of,
};

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
    pub udp_max_peers: usize,
    pub metrics: Option<SocketAddr>,
    pub self_allow: Option<AllowList>,
    pub web: Option<WebSettings>,
}

pub struct ListenClash {
    pub path: Vec<String>,
    pub what: String,
    pub kept: SocketAddr,
}

impl ServerSettings {
    pub fn services_for(&self, client: &str) -> Vec<crate::wire::protocol::ServiceInfo> {
        services_for(&self.services, client)
    }

    pub fn uses_self(&self) -> bool {
        let named = |c: &String| c == burrow_config::SELF;
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
            .or_else(|| {
                next.web
                    .as_ref()
                    .filter(|w| w.https.is_some_and(conflicts))
                    .map(|_| clash(&["web", "https"], "web.https".into()))
            })
            .or_else(|| {
                next.web
                    .as_ref()
                    .filter(|w| w.http.is_some_and(conflicts))
                    .map(|_| clash(&["web", "http"], "web.http".into()))
            })
    }
}

pub fn services_for(
    services: &BTreeMap<String, Service>,
    client: &str,
) -> Vec<crate::wire::protocol::ServiceInfo> {
    services
        .iter()
        .filter(|(_, svc)| svc.clients.iter().any(|c| c == client))
        .map(|(name, svc)| crate::wire::protocol::ServiceInfo {
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
    pub udp_session_timeout: Duration,
    pub udp_max_peers: usize,
    pub forwards: BTreeMap<String, Forward>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forward {
    pub local: SocketAddr,
    pub ports: u16,
    pub to: VisitTarget,
    pub socks: bool,
    pub protocol: Protocol,
}

impl Forward {
    pub fn single(local: SocketAddr, to: VisitTarget) -> Self {
        Forward {
            local,
            ports: 1,
            to,
            socks: false,
            protocol: Protocol::Tcp,
        }
    }

    pub fn udp(local: SocketAddr, to: VisitTarget) -> Self {
        Forward {
            protocol: Protocol::Udp,
            ..Forward::single(local, to)
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
            protocol: Protocol::Tcp,
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
    pub max_connections: Limit,
    pub max_udp_peers: Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub value: u32,
    pub written: bool,
}

impl Limit {
    pub fn default_of(value: u32) -> Limit {
        Limit {
            value,
            written: false,
        }
    }

    pub fn written(value: u32) -> Limit {
        Limit {
            value,
            written: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shares {
    #[default]
    Nothing,
    Https,
    Http,
}

#[derive(Clone, PartialEq)]
pub struct WebSocketSettings {
    pub listen: SocketAddr,
    pub path: String,
    pub tls: Option<TlsMaterial>,
    pub keepalive: std::time::Duration,
    pub host: Option<String>,
    pub shares: Shares,
}

impl WebSocketSettings {
    pub fn shared_url(&self) -> Option<String> {
        let tls = match self.shares {
            Shares::Nothing => return None,
            Shares::Https => true,
            Shares::Http => false,
        };
        let url = Endpoint::WebSocket {
            tls,
            host: self.host.clone()?,
            port: self.listen.port(),
            path: self.path.clone(),
        };
        Some(url.to_string())
    }

    pub fn shape(&self) -> burrow_syntax::address::WsShape<'_> {
        burrow_syntax::address::WsShape {
            listen: self.listen,
            path: &self.path,
            tls: self.tls.is_some(),
        }
    }
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
            .field("host", &self.host)
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
        let secret = crate::wire::secret::Secret::generate().encode();
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
        if cfg!(feature = "web") {
            let web = settings(
                &base.replace("listen: 7835", "listen: 7836\nweb: { https: 0.0.0.0:7835 }"),
            );
            let c = old.listen_clash(&web).unwrap();
            assert_eq!(
                (c.what.as_str(), c.path.clone()),
                ("web.https", vec!["web".to_string(), "https".to_string()])
            );
        }
        let udp = settings(&base.replace("listen: 7835", "listen: 7836").replace(
            "bind: 7840, target: 80",
            "bind: 7835, target: 80, protocol: udp",
        ));
        assert!(
            old.listen_clash(&udp).is_none(),
            "a UDP port does not clash with a TCP listener"
        );
    }

    #[test]
    fn the_shared_tunnel_url_follows_the_web_port_it_shares() {
        let ws = |listen: &str, shares: Shares| WebSocketSettings {
            listen: listen.parse().unwrap(),
            path: "/burrow".into(),
            tls: None,
            keepalive: std::time::Duration::from_secs(25),
            host: Some("tunnel.example.com".into()),
            shares,
        };
        let url = |listen: &str, shares| ws(listen, shares).shared_url();
        assert_eq!(
            url("[::]:443", Shares::Https).as_deref(),
            Some("wss://tunnel.example.com/burrow")
        );
        assert_eq!(
            url("[::]:8443", Shares::Https).as_deref(),
            Some("wss://tunnel.example.com:8443/burrow")
        );
        assert_eq!(
            url("[::]:80", Shares::Http).as_deref(),
            Some("ws://tunnel.example.com/burrow")
        );
        assert_eq!(
            url("[::]:8080", Shares::Http).as_deref(),
            Some("ws://tunnel.example.com:8080/burrow")
        );
        assert_eq!(url("127.0.0.1:7836", Shares::Nothing), None);
    }
}
