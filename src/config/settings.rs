use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use super::model::{Cipher, Protocol};
use crate::allowlist::AllowList;
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
    pub websocket: Option<WebSocketSettings>,
    pub clients: BTreeMap<String, ClientAuth>,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientAuth {
    pub secrets: Vec<Secret>,
    pub max_connections: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Service {
    pub client: String,
    pub bind: SocketAddr,
    pub target: String,
    pub protocol: Protocol,
    pub max_connections: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientSettings {
    pub server: String,
    pub endpoint: Endpoint,
    pub headers: Vec<(String, String)>,
    pub ca_pem: Option<Vec<u8>>,
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
