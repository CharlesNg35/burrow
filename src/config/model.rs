use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "burrow server config")]
pub struct ServerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Where the server waits for clients: ip:port like 0.0.0.0:7835, or a port alone for 0.0.0.0 (all IPv4 addresses)"
    )]
    pub listen: Option<Bind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "How clients reach this server: a host name or IP (the port comes from listen), host:port, or a wss:// URL; client-config writes it into client configs"
    )]
    pub public_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Accept clients over WebSocket, for servers reachable only through HTTP(S)"
    )]
    pub websocket: Option<WebSocketConfig>,
    #[serde(default)]
    #[schemars(description = "Clients allowed to connect, keyed by client name")]
    pub clients: BTreeMap<String, ClientEntry>,
    #[serde(default)]
    #[schemars(description = "Exposed services, keyed by service name")]
    pub services: BTreeMap<String, ServiceEntry>,
    #[serde(default, skip_serializing_if = "is_false")]
    #[schemars(description = "Disable encryption of forwarded data (authentication still runs)")]
    pub insecure_plaintext: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub heartbeat: Heartbeat,
    #[serde(default, skip_serializing_if = "is_default")]
    pub limits: Limits,
    #[serde(default, skip_serializing_if = "is_default")]
    pub pool: Pool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tcp: TcpOptions,
    #[serde(default, skip_serializing_if = "is_default")]
    pub udp: UdpOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebSocketConfig {
    #[schemars(
        description = "Address to accept WebSocket connections on, for example 127.0.0.1:7836 behind a proxy"
    )]
    pub listen: String,
    #[serde(default = "default_ws_path")]
    #[schemars(description = "URL path clients connect to")]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Serve wss:// directly with these PEM files instead of behind a TLS proxy"
    )]
    pub tls: Option<TlsFiles>,
    #[serde(
        default = "default_ws_keepalive",
        skip_serializing_if = "is_default_ws_keepalive"
    )]
    #[schemars(
        description = "Seconds between keepalives on idle pooled channels, below your proxy's idle timeout"
    )]
    #[schemars(range(min = 5, max = 30))]
    pub keepalive_secs: u32,
}

fn default_ws_keepalive() -> u32 {
    25
}

fn is_default_ws_keepalive(v: &u32) -> bool {
    *v == default_ws_keepalive()
}

pub fn default_ws_path() -> String {
    "/burrow".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TlsFiles {
    #[schemars(description = "Certificate chain PEM file, relative to the config file")]
    pub cert: String,
    #[schemars(description = "Private key PEM file, relative to the config file")]
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClientEntry {
    #[schemars(description = "256-bit secret (base64 or hex), or a list of them during rotation")]
    pub secret: Secrets,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Maximum concurrent forwarded connections for this client")]
    pub max_connections: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Secrets {
    One(String),
    Many(Vec<String>),
}

impl Secrets {
    pub fn as_slice(&self) -> &[String] {
        match self {
            Secrets::One(s) => std::slice::from_ref(s),
            Secrets::Many(v) => v,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceEntry {
    #[schemars(description = "Name of the client that reaches the target")]
    pub client: String,
    #[schemars(description = "Public address on the server: a port, or ip:port")]
    pub bind: Bind,
    #[schemars(
        description = "host:port as seen from the client, or a port alone for 127.0.0.1 on the client"
    )]
    pub target: Target,
    #[serde(default, skip_serializing_if = "is_default")]
    pub protocol: Protocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Maximum concurrent connections for this service")]
    pub max_connections: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Bind {
    Port(u16),
    Addr(String),
}

impl std::fmt::Display for Bind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bind::Port(p) => write!(f, "{p}"),
            Bind::Addr(s) => f.write_str(s),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Target {
    Port(u16),
    Addr(String),
}

impl Target {
    pub fn host_port(&self) -> String {
        match self {
            Target::Port(p) => format!("127.0.0.1:{p}"),
            Target::Addr(s) => match s.trim().parse::<u16>() {
                Ok(p) => format!("127.0.0.1:{p}"),
                Err(_) => s.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Tcp,
    Udp,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Heartbeat {
    #[schemars(range(min = 1, max = 3600))]
    pub interval_secs: u32,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Heartbeat { interval_secs: 10 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    #[schemars(range(min = 1, max = 120))]
    pub handshake_timeout_secs: u32,
    #[schemars(description = "Concurrent connections that have not finished authenticating")]
    #[schemars(range(min = 1))]
    pub max_handshakes: u32,
    #[schemars(range(min = 1))]
    pub max_connections_per_client: u32,
    #[schemars(description = "Visitors waiting for a data channel, per client")]
    #[schemars(range(min = 1))]
    pub max_pending: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            handshake_timeout_secs: 5,
            max_handshakes: 256,
            max_connections_per_client: 8192,
            max_pending: 512,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Pool {
    #[schemars(range(max = 1024))]
    pub min_idle: u32,
    #[schemars(range(min = 1, max = 4096))]
    pub max_idle: u32,
}

impl Default for Pool {
    fn default() -> Self {
        Pool {
            min_idle: 4,
            max_idle: 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TcpOptions {
    pub nodelay: bool,
    #[schemars(description = "TCP keepalive idle time, 0 disables")]
    pub keepalive_secs: u32,
}

impl Default for TcpOptions {
    fn default() -> Self {
        TcpOptions {
            nodelay: true,
            keepalive_secs: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct UdpOptions {
    #[schemars(range(min = 1))]
    pub session_timeout_secs: u32,
}

impl Default for UdpOptions {
    fn default() -> Self {
        UdpOptions {
            session_timeout_secs: 60,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "burrow client config")]
pub struct ClientConfig {
    #[schemars(
        description = "Server address: host or host:port for TCP (port 7835 when omitted), or ws://host/path or wss://host/path"
    )]
    pub server: String,
    #[schemars(description = "This client's name, as defined in the server config")]
    pub name: String,
    #[schemars(description = "256-bit secret (base64 or hex) shared with the server")]
    pub secret: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Targets the server may reach: CIDR, optionally with :port or :lo-hi"
    )]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    #[schemars(description = "Allow the server to reach any target (not recommended)")]
    pub allow_all: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub insecure_plaintext: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Extra HTTP headers for the WebSocket upgrade, for example an access token for a proxy"
    )]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "CA certificate, or the server certificate itself (pinned), to trust for wss://; relative to the config file"
    )]
    pub ca: Option<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub cipher: Cipher,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Connect through a proxy: http://host:port or socks5://[user:pass@]host:port"
    )]
    pub proxy: Option<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub reconnect: Reconnect,
    #[serde(
        default = "default_connect_timeout",
        skip_serializing_if = "is_default_connect_timeout"
    )]
    #[schemars(range(min = 1, max = 300))]
    pub connect_timeout_secs: u32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tcp: TcpOptions,
    #[serde(default, skip_serializing_if = "is_default")]
    pub udp: UdpOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Cipher {
    #[default]
    Chachapoly,
    AesGcm,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Reconnect {
    #[schemars(range(min = 10, max = 60000))]
    pub initial_delay_ms: u32,
    #[schemars(range(min = 1, max = 3600))]
    pub max_delay_secs: u32,
    #[schemars(
        description = "Stop retrying after the server has been unreachable this long; 0 retries forever"
    )]
    pub give_up_after_secs: u32,
    #[schemars(description = "What to do when the server rejects the secret or name")]
    pub on_rejected: OnRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OnRejected {
    #[default]
    Retry,
    Exit,
}

impl Default for Reconnect {
    fn default() -> Self {
        Reconnect {
            initial_delay_ms: 250,
            max_delay_secs: 30,
            give_up_after_secs: 0,
            on_rejected: OnRejected::Retry,
        }
    }
}

fn default_connect_timeout() -> u32 {
    10
}

fn is_default_connect_timeout(v: &u32) -> bool {
    *v == default_connect_timeout()
}

fn is_false(v: &bool) -> bool {
    !v
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}
