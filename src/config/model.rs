use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::admin_schema::AdminSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "burrow server config")]
#[admin(
    label = "Server",
    groups(
        listener = "Listener",
        websocket = "WebSocket",
        entities = "Clients, visitors and services",
        limits = "Limits",
        network = "Network",
        monitoring = "Monitoring",
        security = "Security"
    )
)]
pub struct ServerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Where the server waits for clients: ip:port like 0.0.0.0:7835, or a port alone for 0.0.0.0 (all IPv4 addresses)"
    )]
    #[admin(
        label = "Listen address",
        group = listener,
        applies = restart,
        none_label = "No TCP listener",
        locked
    )]
    pub listen: Option<Bind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "How clients reach this server: a host name or IP (the port comes from listen), host:port, or a wss:// URL; client-config writes it into client configs"
    )]
    #[admin(
        ty = server_address,
        group = listener,
        applies = live,
        none_label = "Not set"
    )]
    pub public_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Accept clients over WebSocket, for servers reachable only through HTTP(S)"
    )]
    #[admin(
        label = "WebSocket listener",
        group = websocket,
        applies = reload,
        none_label = "Off",
        locked
    )]
    pub websocket: Option<WebSocketConfig>,
    #[serde(default)]
    #[schemars(description = "Clients allowed to connect, keyed by client name")]
    #[admin(ty = map(client), group = entities, applies = reload)]
    pub clients: BTreeMap<String, ClientEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Visitors allowed to reach private services with burrow visit, keyed by visitor name"
    )]
    #[admin(ty = map(visitor), group = entities, applies = reload)]
    pub visitors: BTreeMap<String, VisitorEntry>,
    #[serde(default)]
    #[schemars(description = "Exposed services, keyed by service name")]
    #[admin(ty = map(service), group = entities, applies = reload)]
    pub services: BTreeMap<String, ServiceEntry>,
    #[serde(default, skip_serializing_if = "is_false")]
    #[schemars(description = "Disable encryption of forwarded data (authentication still runs)")]
    #[admin(
        label = "Disable encryption of forwarded data",
        group = security,
        applies = live,
        danger_when_set,
        locked
    )]
    pub insecure_plaintext: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(group = network, applies = live)]
    pub heartbeat: Heartbeat,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(group = limits)]
    pub limits: Limits,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(label = "Pool of idle channels", group = network, applies = reload)]
    pub pool: Pool,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(label = "TCP", group = network, applies = live)]
    pub tcp: TcpOptions,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(label = "UDP", group = network, applies = live)]
    pub udp: UdpOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Serve Prometheus metrics at /metrics; off unless set")]
    #[admin(
        label = "Metrics endpoint",
        group = monitoring,
        applies = reload,
        none_label = "Off"
    )]
    pub metrics: Option<MetricsConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields)]
#[admin(label = "Metrics")]
pub struct MetricsConfig {
    #[schemars(
        description = "Address for the metrics endpoint, for example 127.0.0.1:9100; a port alone means 127.0.0.1"
    )]
    #[admin(ty = ip_port(default_ip = "127.0.0.1"), label = "Listen address")]
    pub listen: Bind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields)]
#[admin(label = "WebSocket")]
pub struct WebSocketConfig {
    #[schemars(
        description = "Address to accept WebSocket connections on, for example 127.0.0.1:7836 behind a proxy; a port alone means 0.0.0.0"
    )]
    #[admin(label = "Listen address")]
    pub listen: Bind,
    #[serde(default = "default_ws_path")]
    #[schemars(description = "URL path clients connect to")]
    #[admin(ty = url_path)]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Serve wss:// directly with these PEM files instead of behind a TLS proxy"
    )]
    #[admin(label = "TLS", none_label = "Behind a TLS proxy (plain ws://)")]
    pub tls: Option<TlsFiles>,
    #[serde(
        default = "default_ws_keepalive",
        skip_serializing_if = "is_default_ws_keepalive"
    )]
    #[schemars(
        description = "Seconds between keepalives on idle pooled channels, below your proxy's idle timeout"
    )]
    #[schemars(range(min = 5, max = 30))]
    #[admin(label = "Keepalive", advanced)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields)]
#[admin(label = "TLS files")]
pub struct TlsFiles {
    #[schemars(description = "Certificate chain PEM file, relative to the config file")]
    #[admin(ty = path(must_exist, relative_to = "config"), label = "Certificate file")]
    pub cert: String,
    #[schemars(description = "Private key PEM file, relative to the config file")]
    #[admin(ty = path(must_exist, relative_to = "config"), label = "Private key file")]
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields)]
#[admin(entity = client, label = "Client")]
pub struct ClientEntry {
    #[schemars(description = "256-bit secret (base64 or hex), or a list of them during rotation")]
    #[admin(label = "Secrets")]
    pub secret: Secrets,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Maximum concurrent forwarded connections for this client")]
    #[schemars(range(min = 1))]
    #[admin(
        ty = integer(unit = "connections"),
        none_label = "Server default",
        default_from("/limits/max_connections_per_client"),
        advanced
    )]
    pub max_connections: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields)]
#[admin(entity = visitor, label = "Visitor", groups(access = "Access", admin = "Administration", traffic = "Traffic"))]
pub struct VisitorEntry {
    #[schemars(description = "256-bit secret (base64 or hex), or a list of them during rotation")]
    #[admin(label = "Secrets")]
    pub secret: Secrets,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Clients this visitor may reach directly, with the targets it may pick there: any (whatever the client's allow list permits), or rules in allow-list syntax that narrow it, for example home: any, office: 192.168.150.0/25 or lab: [10.0.0.0/8:22, unix:/run/app.sock]"
    )]
    #[admin(
        ty = ref_map(client),
        label = "Direct access to clients",
        group = access,
        empty_label = "No direct access"
    )]
    pub reach: BTreeMap<String, Reach>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Maximum concurrent connections for this visitor")]
    #[schemars(range(min = 1))]
    #[admin(
        ty = integer(unit = "connections"),
        group = traffic,
        none_label = "Unlimited",
        advanced
    )]
    pub max_connections: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Let this visitor administer the server with burrow admin: read shows the dashboard and the config, write may also change it"
    )]
    #[admin(
        label = "Admin access",
        group = admin,
        none_label = "None",
        danger_when_set
    )]
    pub admin: Option<AdminLevel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Admin access only from these source IPs or CIDRs, for example [203.0.113.0/24]; any address when empty"
    )]
    #[admin(
        ty = list(cidr),
        label = "Admin only from",
        group = admin,
        visible_if(set(admin)),
        empty_label = "Any address"
    )]
    pub admin_from: Vec<String>,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
    AdminSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum AdminLevel {
    #[admin(
        label = "Read",
        help = "Dashboard, config with secrets as fingerprints, history"
    )]
    Read,
    #[admin(
        label = "Write",
        help = "Everything read gives, and changing the config",
        danger
    )]
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(untagged)]
#[admin(semantic = union(enum_values("any"), cidr_rule, list(cidr_rule)))]
pub enum Reach {
    #[admin(label = "Any or one rule")]
    One(String),
    #[admin(label = "Rules")]
    Many(Vec<String>),
}

impl Reach {
    pub fn entries(&self) -> &[String] {
        match self {
            Reach::One(s) => std::slice::from_ref(s),
            Reach::Many(v) => v,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(untagged)]
#[admin(semantic = secret_ref(max = 4))]
pub enum Secrets {
    #[admin(label = "One secret")]
    One(String),
    #[admin(label = "Several secrets during rotation")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields)]
#[admin(
    entity = service,
    tcp_only(mode, health_check, max_connections, visitors, proxy_protocol),
    label = "Service",
    groups(
        who = "Where it runs",
        exposure = "Exposure",
        access = "Access",
        traffic = "Traffic"
    )
)]
pub struct ServiceEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Name of the client that reaches the target")]
    #[admin(ty = ref_(client), group = who, one_of = "who", none_label = "None")]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Several clients that can each reach the target, instead of client; see mode"
    )]
    #[admin(ty = ref_list(client, ordered), group = who, one_of = "who")]
    pub clients: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    #[schemars(
        description = "With several clients: failover uses the first one that is up, balance spreads new connections over all of them"
    )]
    #[admin(group = who, visible_if(len_ge(clients, 2)))]
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Check that each client can reach the target, and send connections only to those that can: true, or a map with interval_secs, timeout_secs and failures"
    )]
    #[admin(
        group = who,
        none_label = "Off",
        advanced
    )]
    pub health_check: Option<HealthCheckSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Public address on the server: a port, or ip:port; leave it out for a private service that only visitors reach"
    )]
    #[admin(
        ty = ip_port(ranges, max_range = 1000),
        label = "Public address",
        group = exposure,
        none_label = "Private: no public port",
        cross(target, protocol)
    )]
    pub bind: Option<Bind>,
    #[schemars(
        description = "host:port as seen from the client, or a port alone for 127.0.0.1 on the client"
    )]
    #[admin(
        ty = target(ranges, unix),
        label = "Target on the client",
        group = exposure,
        same_len(bind),
        cross(bind, protocol)
    )]
    pub target: Target,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(group = exposure)]
    pub protocol: Protocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Maximum concurrent connections for this service")]
    #[schemars(range(min = 1))]
    #[admin(
        ty = integer(unit = "connections"),
        group = traffic,
        none_label = "Unlimited"
    )]
    pub max_connections: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(description = "Visitors that may reach this service with burrow visit")]
    #[admin(
        ty = ref_list(visitor),
        group = access,
        empty_label = "No visitors",
        inverse(entity = visitor, label = "Services it may use"),
        options_filter(eq(protocol, "tcp"))
    )]
    pub visitors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Only these source IPs or CIDRs may connect, for example [203.0.113.0/24]; everyone when empty"
    )]
    #[admin(ty = list(cidr), label = "Only from", group = access, empty_label = "Everyone may connect")]
    pub allow_from: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(description = "Source IPs or CIDRs that may never connect")]
    #[admin(ty = list(cidr), label = "Never from", group = access, empty_label = "Nobody is blocked", advanced)]
    pub deny_from: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Send the real source address to the target with the PROXY protocol, v1 (text) or v2 (binary); only for targets that expect it"
    )]
    #[admin(
        label = "PROXY protocol",
        group = traffic,
        none_label = "Off",
        advanced
    )]
    pub proxy_protocol: Option<crate::proxy_protocol::Version>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Speed limit for each direction, shared by all connections of this service, for example 40mbit or 5MB (per second)"
    )]
    #[admin(ty = rate, label = "Speed limit", group = traffic, none_label = "Unlimited")]
    pub bandwidth: Option<String>,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    AdminSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    #[admin(label = "Failover", help = "Use the first client that is up")]
    Failover,
    #[admin(label = "Balance", help = "Spread new connections over all clients")]
    Balance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(untagged)]
#[admin(semantic = union(bool, object(HealthCheck)))]
pub enum HealthCheckSetting {
    #[admin(label = "On with defaults")]
    On(bool),
    #[admin]
    Tuned(HealthCheck),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields, default)]
#[admin(label = "Health check settings")]
pub struct HealthCheck {
    #[schemars(description = "Seconds between checks of each client")]
    #[schemars(range(min = 1, max = 3600))]
    #[admin(label = "Interval")]
    pub interval_secs: u32,
    #[schemars(description = "Seconds a check may take")]
    #[schemars(range(min = 1, max = 60))]
    #[admin(label = "Timeout")]
    pub timeout_secs: u32,
    #[schemars(description = "Failed checks in a row before a client counts as down")]
    #[schemars(range(min = 1, max = 100))]
    #[admin(ty = integer(unit = "checks"), label = "Failures before down")]
    pub failures: u32,
}

impl Default for HealthCheck {
    fn default() -> Self {
        HealthCheck {
            interval_secs: 10,
            timeout_secs: 3,
            failures: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(untagged)]
#[admin(semantic = ip_port)]
pub enum Bind {
    #[admin]
    Port(#[schemars(range(min = 1))] u16),
    #[admin(label = "Text")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(untagged)]
#[admin(semantic = target)]
pub enum Target {
    #[admin]
    Port(#[schemars(range(min = 1))] u16),
    #[admin(label = "Text")]
    Addr(String),
}

impl Target {
    pub fn host_port(&self) -> String {
        match self {
            Target::Port(p) => format!("127.0.0.1:{p}"),
            Target::Addr(s) => {
                let s = s.trim();
                if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '-') {
                    format!("127.0.0.1:{s}")
                } else {
                    s.to_string()
                }
            }
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    AdminSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    #[admin(label = "TCP")]
    Tcp,
    #[admin(label = "UDP")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields, default)]
#[admin(label = "Heartbeat")]
pub struct Heartbeat {
    #[schemars(range(min = 1, max = 3600))]
    #[admin(
        label = "Interval",
        help = "How often the server and each client check that the other is still there",
        locked
    )]
    pub interval_secs: u32,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Heartbeat { interval_secs: 10 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields, default)]
#[admin(label = "Limits")]
pub struct Limits {
    #[schemars(range(min = 1, max = 120))]
    #[admin(
        label = "Handshake timeout",
        help = "Time a new connection has to authenticate",
        applies = live,
        locked
    )]
    pub handshake_timeout_secs: u32,
    #[schemars(description = "Concurrent connections that have not finished authenticating")]
    #[schemars(range(min = 1, max = 1000000))]
    #[admin(ty = integer(unit = "connections"), applies = reload, locked)]
    pub max_handshakes: u32,
    #[schemars(range(min = 1))]
    #[admin(
        ty = integer(unit = "connections"),
        help = "Concurrent forwarded connections of one client, unless the client sets its own",
        applies = live
    )]
    pub max_connections_per_client: u32,
    #[schemars(description = "Connections waiting for a data channel, per client")]
    #[schemars(range(min = 1))]
    #[admin(ty = integer(unit = "connections"), label = "Max waiting connections", applies = reload)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields, default)]
#[admin(label = "Pool")]
pub struct Pool {
    #[schemars(range(max = 1024))]
    #[admin(
        ty = integer(unit = "channels"),
        label = "Idle channels kept ready",
        help = "Data channels each client keeps open in advance, so a new connection starts without a round trip"
    )]
    pub min_idle: u32,
    #[schemars(range(min = 1, max = 4096))]
    #[admin(ty = integer(unit = "channels"), label = "Max idle channels")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields, default)]
#[admin(label = "TCP options")]
pub struct TcpOptions {
    #[admin(
        label = "No delay",
        help = "Send small writes at once instead of batching them (TCP_NODELAY)"
    )]
    pub nodelay: bool,
    #[schemars(
        description = "TCP keepalive idle time, 0 disables",
        range(max = 32767)
    )]
    #[admin(label = "Keepalive")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields, default)]
#[admin(label = "UDP options")]
pub struct UdpOptions {
    #[schemars(range(min = 1))]
    #[admin(
        label = "Session timeout",
        help = "A UDP peer that sends nothing for this long is forgotten"
    )]
    pub session_timeout_secs: u32,
}

impl Default for UdpOptions {
    fn default() -> Self {
        UdpOptions {
            session_timeout_secs: 60,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "burrow client config")]
#[admin(skip = "edited on that machine")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, AdminSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "burrow visitor config")]
#[admin(skip = "edited on that machine")]
pub struct VisitorConfig {
    #[schemars(
        description = "Server address: host or host:port for TCP (port 7835 when omitted), or ws://host/path or wss://host/path"
    )]
    pub server: String,
    #[schemars(
        description = "This visitor's name, as defined under visitors in the server config"
    )]
    pub name: String,
    #[schemars(description = "256-bit secret (base64 or hex) shared with the server")]
    pub secret: String,
    #[serde(default)]
    #[schemars(
        description = "Local ports that lead to private services or client targets, keyed by forward name"
    )]
    pub forwards: BTreeMap<String, ForwardEntry>,
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
    #[serde(
        default = "default_connect_timeout",
        skip_serializing_if = "is_default_connect_timeout"
    )]
    #[schemars(range(min = 1, max = 300))]
    pub connect_timeout_secs: u32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tcp: TcpOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Private service on the server to reach")]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Client to reach directly, with target; needs reach for this visitor in the server config"
    )]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "host:port as seen from the client, or a port alone for 127.0.0.1 on the client"
    )]
    pub target: Option<Target>,
    #[serde(default, skip_serializing_if = "is_false")]
    #[schemars(
        description = "With client: listen as a SOCKS5 proxy, so each connection names its own target on that client"
    )]
    pub socks: bool,
    #[schemars(
        description = "Where to listen on this machine: a port alone for 127.0.0.1, or ip:port"
    )]
    pub local: Bind,
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
        description = "Stop retrying after this long without a working connection to the server; 0 retries forever"
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
