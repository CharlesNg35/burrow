use std::collections::BTreeMap;

use burrow_admin_proto::msg::AdminLevel;
use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[schemars(title = "burrow server config")]
#[admin(
    label = "Server",
    groups(
        listener = "Listener",
        websocket = "WebSocket",
        entities = "Clients, visitors and services",
        local = "This server | What this server may dial for itself, for services with client: self and reach: self.",
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
    #[admin(help = "Where the server waits for clients, for example 0.0.0.0:7835, or just a port such as 7835 for every IPv4 address",
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
    #[admin(help = "How clients reach this server, for example tunnel.example.com or tunnel.example.com:7835, or a wss:// URL such as wss://tunnel.example.com/burrow when WebSocket is on; the connect commands and client configs made here use it",
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
    #[admin(help = "Clients allowed to connect", ty = map(client), group = entities, applies = reload)]
    pub clients: BTreeMap<String, ClientEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Visitors allowed to reach private services with burrow visit, keyed by visitor name"
    )]
    #[admin(help = "Visitors allowed to reach private services with burrow visit", ty = map(visitor), group = entities, applies = reload)]
    pub visitors: BTreeMap<String, VisitorEntry>,
    #[serde(rename = "self", default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Targets this server may dial for itself, for services with client: self and visitors with reach: self"
    )]
    #[admin(help = "Targets this server may reach itself, for services that run on this server and visitors that reach it directly",
        label = "This server",
        group = local,
        applies = reload,
        none_label = "Only the targets of its own services"
    )]
    pub self_: Option<SelfEntry>,
    #[serde(default)]
    #[schemars(description = "Exposed services, keyed by service name")]
    #[admin(help = "Services this server exposes", ty = map(service), group = entities, applies = reload)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "Metrics")]
pub struct MetricsConfig {
    #[schemars(
        description = "Address for the metrics endpoint, for example 127.0.0.1:9100; a port alone means 127.0.0.1"
    )]
    #[admin(ty = ip_port(default_ip = "127.0.0.1"), label = "Listen address")]
    pub listen: Bind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(label = "This server")]
pub struct SelfEntry {
    #[serde(default)]
    #[schemars(
        description = "What this server may dial for itself: CIDR, optionally with :port or :lo-hi, or unix:/path; link-local and metadata addresses only when listed one by one"
    )]
    #[admin(help = "Addresses this server may reach itself, with optional ports, or Unix sockets, for example 10.0.0.0/8, 192.168.1.10:22 or unix:/run/app.sock; link-local and cloud metadata addresses only when listed one by one", ty = list(cidr_rule), label = "Allowed targets", empty_label = "Nothing")]
    pub allow: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(entity = client, label = "Client")]
pub struct ClientEntry {
    #[schemars(description = "256-bit secret (base64 or hex), or a list of them during rotation")]
    #[admin(
        help = "Secrets this client may sign in with; it has several only during a rotation",
        label = "Secrets"
    )]
    pub secret: Secrets,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Maximum concurrent forwarded connections for this client")]
    #[schemars(range(min = 1))]
    #[admin(
        ty = integer(unit = "connections"),
        label = "Max connections on this server",
        help = "Connections this server forwards to the client at once. The client has its own limit too, max_connections in its client.yaml",
        none_label = "Server default",
        default_from("/limits/max_connections_per_client"),
        advanced
    )]
    pub max_connections: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(entity = visitor, label = "Visitor", groups(
    admin = "Admin access | May this visitor run burrow admin against this server?",
    access = "Access | Private services and direct reach.",
    traffic = "Traffic | Limits for this visitor."
))]
pub struct VisitorEntry {
    #[schemars(description = "256-bit secret (base64 or hex), or a list of them during rotation")]
    #[admin(
        help = "Secrets this visitor may sign in with; it has several only during a rotation",
        label = "Secrets",
        reduce_only
    )]
    pub secret: Secrets,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Clients this visitor may reach directly, over TCP and UDP, with the targets it may pick there: any (whatever the client's allow list permits), or rules in allow-list syntax that narrow it, for example home: any, office: 192.168.150.0/25 or lab: [10.0.0.0/8:22, unix:/run/app.sock]"
    )]
    #[admin(help = "Clients this visitor may reach directly, over TCP and UDP, and which targets there: any target the client allows, or only the ones listed, for example 192.168.150.0/25 or 10.0.0.5:22",
        ty = ref_map(client, reserved = "self"),
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
        danger_when_set,
        reduce_only,
        wide
    )]
    pub admin: Option<AdminLevel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Admin access only from these source IPs or CIDRs, for example [203.0.113.0/24]; any address when empty"
    )]
    #[admin(help = "Admin access only from these addresses or networks, for example 203.0.113.0/24 or 10.0.0.0/8; any address when empty",
        ty = list(cidr),
        label = "Admin only from",
        group = admin,
        visible_if(set(admin)),
        empty_label = "Any address",
        reduce_only
    )]
    pub admin_from: Vec<String>,
}

#[derive(Serialize, AdminSchema)]
#[admin(remote = AdminLevel)]
#[serde(rename_all = "lowercase")]
enum AdminLevelSchema {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "json-schema", schemars(rename = "Version"))]
#[serde(rename_all = "lowercase")]
pub enum ProxyVersion {
    #[admin(label = "v1 (text)")]
    V1,
    #[admin(label = "v2 (binary)")]
    V2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[admin(
    entity = service,
    tcp_only(mode, health_check, max_connections, proxy_protocol),
    label = "Service",
    groups(
        who = "Where it runs | Clients that reach the target.",
        exposure = "Exposure | What opens on the server, and where it leads.",
        access = "Access | Who may connect.",
        traffic = "Traffic | Limits and what the target sees."
    )
)]
pub struct ServiceEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Name of the client that reaches the target")]
    #[admin(ty = ref_(client, reserved = "self"), group = who, one_of = "who", none_label = "None")]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Several clients that can each reach the target, instead of client; see mode"
    )]
    #[admin(help = "Clients that can each reach the target; with two or more, Mode appears to choose how one is picked", ty = ref_list(client, ordered, reserved = "self"), group = who, one_of = "who")]
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
    #[admin(help = "Check that each client can reach the target, and send connections only to those that can",
        group = who,
        none_label = "Off",
        advanced
    )]
    pub health_check: Option<HealthCheckSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Public address on the server: a port, or ip:port; leave it out for a private service that only visitors reach"
    )]
    #[admin(help = "The address and port the server opens for this service, for example 8080 or 127.0.0.1:8080",
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
    #[admin(help = "Where the client connects, as seen from the client, for example 127.0.0.1:3000 or 10.0.0.5:22; a port alone means the client itself",
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
        narrow
    )]
    pub visitors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Only these source IPs or CIDRs may connect, for example [203.0.113.0/24]; everyone when empty"
    )]
    #[admin(help = "Only these addresses or networks may connect, for example 203.0.113.0/24 or 10.0.0.0/8", ty = list(cidr), group = access, empty_label = "Empty means everyone may connect")]
    pub allow_from: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(description = "Source IPs or CIDRs that may never connect")]
    #[admin(help = "Addresses or networks that may never connect, for example 198.51.100.7 or 198.51.100.0/24",
        ty = list(cidr),
        group = access,
        empty_label = "No addresses denied",
        advanced,
        narrow
    )]
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
    pub proxy_protocol: Option<ProxyVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Speed limit for each direction, shared by all connections of this service, for example 40mbit or 5MB (per second)"
    )]
    #[admin(help = "Speed limit in each direction, shared by all connections of this service, for example 40mbit or 5MB per second",
        ty = rate,
        label = "Speed limit",
        group = traffic,
        none_label = "Unlimited",
        wide
    )]
    pub bandwidth: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    #[admin(label = "Failover", help = "Use the first client that is up")]
    Failover,
    #[admin(label = "Balance", help = "Spread new connections over all clients")]
    Balance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[admin(semantic = union(bool, object(HealthCheck)))]
pub enum HealthCheckSetting {
    #[admin(label = "On with defaults")]
    On(bool),
    #[admin]
    Tuned(HealthCheck),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    #[admin(label = "TCP")]
    Tcp,
    #[admin(label = "UDP")]
    Udp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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
    #[admin(
        help = "Idle time before keepalive probes start; 0 turns them off",
        label = "Keepalive"
    )]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, default)]
#[admin(label = "UDP options")]
pub struct UdpOptions {
    #[schemars(range(min = 1))]
    #[admin(
        label = "Session timeout",
        help = "A UDP peer that sends nothing for this long is forgotten"
    )]
    pub session_timeout_secs: u32,
    #[schemars(
        description = "Most UDP peers one service port keeps at once; a datagram from a new peer beyond it is dropped",
        range(min = 1, max = 1000000),
        extend("x-show-default" = true)
    )]
    #[admin(
        ty = integer(unit = "peers"),
        label = "Max UDP peers per port",
        help = "Most UDP peers one service port keeps at once; a datagram from a new peer beyond it is dropped. For example 20000; the default is 4096",
        applies = live
    )]
    pub max_peers: u32,
}

impl Default for UdpOptions {
    fn default() -> Self {
        UdpOptions {
            session_timeout_secs: 120,
            max_peers: 4096,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, default)]
pub struct ClientUdpOptions {
    #[cfg_attr(feature = "json-schema", schemars(range(min = 1)))]
    pub session_timeout_secs: u32,
}

impl Default for ClientUdpOptions {
    fn default() -> Self {
        ClientUdpOptions {
            session_timeout_secs: 120,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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
    #[serde(
        default = "default_client_max_connections",
        skip_serializing_if = "is_default_client_max_connections"
    )]
    #[schemars(
        description = "Most connections open through this client at once; beyond it the server gets busy. Lowered automatically when the open file limit is too small",
        range(min = 1, max = 1000000),
        extend("default" = CLIENT_MAX_CONNECTIONS, "x-show-default" = true)
    )]
    pub max_connections: u32,
    #[serde(
        default = "default_client_max_udp_peers",
        skip_serializing_if = "is_default_client_max_udp_peers"
    )]
    #[schemars(
        description = "Most UDP peers open through this client at once, across all UDP services; a datagram from a new peer beyond it is dropped. Lowered automatically when the open file limit is too small",
        range(min = 1, max = 1000000),
        extend("default" = CLIENT_MAX_UDP_PEERS, "x-show-default" = true)
    )]
    pub max_udp_peers: u32,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tcp: TcpOptions,
    #[serde(default, skip_serializing_if = "is_default")]
    pub udp: ClientUdpOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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
    #[serde(default, skip_serializing_if = "is_default")]
    #[schemars(
        description = "UDP forwards: how long a local UDP peer is kept without traffic, and how many peers each forward port keeps"
    )]
    pub udp: VisitorUdpOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, default)]
pub struct VisitorUdpOptions {
    #[cfg_attr(
        feature = "json-schema",
        schemars(
            description = "A local UDP peer of a UDP forward that sends and gets nothing for this long is forgotten",
            range(min = 1)
        )
    )]
    pub session_timeout_secs: u32,
    #[cfg_attr(
        feature = "json-schema",
        schemars(
            description = "Most UDP peers each port of a UDP forward keeps at once; a datagram from a new peer beyond it is dropped",
            range(min = 1, max = 1000000)
        )
    )]
    pub max_peers: u32,
}

impl Default for VisitorUdpOptions {
    fn default() -> Self {
        let server = UdpOptions::default();
        VisitorUdpOptions {
            session_timeout_secs: server.session_timeout_secs,
            max_peers: server.max_peers,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ForwardEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "json-schema",
        schemars(description = "Private service on the server to reach")
    )]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "json-schema",
        schemars(
            description = "Client to reach directly, with target; needs reach for this visitor in the server config"
        )
    )]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "json-schema",
        schemars(
            description = "host:port as seen from the client, or a port alone for 127.0.0.1 on the client"
        )
    )]
    pub target: Option<Target>,
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(
        feature = "json-schema",
        schemars(
            description = "With client: listen as a SOCKS5 proxy, so each connection names its own target on that client"
        )
    )]
    pub socks: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    #[cfg_attr(
        feature = "json-schema",
        schemars(
            description = "tcp or udp; must match the service's protocol on the server, and reach covers both"
        )
    )]
    pub protocol: Protocol,
    #[cfg_attr(
        feature = "json-schema",
        schemars(
            description = "Where to listen on this machine: a port alone for 127.0.0.1, or ip:port"
        )
    )]
    pub local: Bind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Cipher {
    #[default]
    Chachapoly,
    AesGcm,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, default)]
pub struct Reconnect {
    #[cfg_attr(feature = "json-schema", schemars(range(min = 10, max = 60000)))]
    pub initial_delay_ms: u32,
    #[cfg_attr(feature = "json-schema", schemars(range(min = 1, max = 3600)))]
    pub max_delay_secs: u32,
    #[cfg_attr(
        feature = "json-schema",
        schemars(
            description = "Stop retrying after this long without a working connection to the server; 0 retries forever"
        )
    )]
    pub give_up_after_secs: u32,
    #[cfg_attr(
        feature = "json-schema",
        schemars(description = "What to do when the server rejects the secret or name")
    )]
    pub on_rejected: OnRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

pub const CLIENT_MAX_CONNECTIONS: u32 = 16384;
pub const CLIENT_MAX_UDP_PEERS: u32 = 8192;

fn default_client_max_udp_peers() -> u32 {
    CLIENT_MAX_UDP_PEERS
}

fn is_default_client_max_udp_peers(v: &u32) -> bool {
    *v == CLIENT_MAX_UDP_PEERS
}

fn default_client_max_connections() -> u32 {
    CLIENT_MAX_CONNECTIONS
}

fn is_default_client_max_connections(v: &u32) -> bool {
    *v == CLIENT_MAX_CONNECTIONS
}

fn is_false(v: &bool) -> bool {
    !v
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_schema::AdminEnum;
    use burrow_admin_proto::serde_json::{self, json};

    #[test]
    fn the_admin_level_options_are_its_wire_values() {
        let options = AdminLevel::options();
        assert_eq!(options.len(), 2);
        for o in options {
            let l: AdminLevel = serde_json::from_value(json!(o.value)).unwrap();
            assert_eq!(l.wire(), o.value);
            assert_eq!(AdminLevel::from_wire(&o.value), Some(l));
            assert_eq!(serde_json::to_value(l).unwrap(), json!(o.value));
        }
        assert_eq!(AdminLevel::from_wire("owner"), None);
    }

    #[test]
    fn a_client_keeps_16384_connections_unless_told_otherwise() {
        let base = json!({"server": "tunnel.example.com", "name": "home", "secret": "x"});
        let plain: ClientConfig = serde_json::from_value(base.clone()).unwrap();
        assert_eq!(plain.max_connections, 16384);
        assert!(
            serde_json::to_value(&plain)
                .unwrap()
                .get("max_connections")
                .is_none()
        );
        let mut set = base;
        set["max_connections"] = json!(2);
        let two: ClientConfig = serde_json::from_value(set).unwrap();
        assert_eq!(two.max_connections, 2);
        assert_eq!(serde_json::to_value(&two).unwrap()["max_connections"], 2);
    }

    #[test]
    fn a_client_keeps_8192_udp_peers_unless_told_otherwise() {
        let base = json!({"server": "tunnel.example.com", "name": "home", "secret": "x"});
        let plain: ClientConfig = serde_json::from_value(base.clone()).unwrap();
        assert_eq!(plain.max_udp_peers, 8192);
        assert!(
            serde_json::to_value(&plain)
                .unwrap()
                .get("max_udp_peers")
                .is_none()
        );
        let mut set = base;
        set["max_udp_peers"] = json!(3);
        let three: ClientConfig = serde_json::from_value(set).unwrap();
        assert_eq!(three.max_udp_peers, 3);
        assert_eq!(serde_json::to_value(&three).unwrap()["max_udp_peers"], 3);
        let udp: Result<ClientConfig, _> = serde_json::from_value(
            json!({"server": "a", "name": "b", "secret": "c", "udp": {"max_peers": 5}}),
        );
        assert!(udp.is_err());
    }

    #[test]
    fn a_server_keeps_4096_udp_peers_per_port_unless_told_otherwise() {
        let plain = UdpOptions::default();
        assert_eq!(plain.max_peers, 4096);
        let set: UdpOptions = serde_json::from_value(json!({"max_peers": 7})).unwrap();
        assert_eq!(set.max_peers, 7);
        assert_eq!(serde_json::to_value(&set).unwrap()["max_peers"], 7);
    }

    #[test]
    fn udp_peers_are_kept_two_minutes_by_default_as_rfc_4787_req_5_asks() {
        assert_eq!(UdpOptions::default().session_timeout_secs, 120);
        assert_eq!(ClientUdpOptions::default().session_timeout_secs, 120);
        assert_eq!(VisitorUdpOptions::default().session_timeout_secs, 120);
        assert_eq!(VisitorUdpOptions::default().max_peers, 4096);
        let set: UdpOptions = serde_json::from_value(json!({"max_peers": 7})).unwrap();
        assert_eq!(set.session_timeout_secs, 120);
    }
}
