use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

use super::ChallengeKind;
use super::{
    Bind, ClientEntry, Heartbeat, Limits, Pool, ServiceEntry, TcpOptions, UdpOptions, VisitorEntry,
    WebConfig, is_default, is_false,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[schemars(title = "burrow server config")]
#[admin(
    label = "Server",
    groups(
        listener = "Listener",
        websocket = "WebSocket",
        web = "Web proxy | One HTTPS port for websites and TLS services, told apart by name.",
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
    #[admin(help = "How clients reach this server, for example tunnel.example.com, or wss://tunnel.example.com/burrow when WebSocket is on; connect commands made here use it",
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
        none_label = "Off"
    )]
    pub websocket: Option<WebSocketConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The web proxy: one HTTPS port for websites and TLS services by name; off unless set"
    )]
    #[admin(
        label = "Web proxy",
        group = web,
        applies = reload,
        none_label = "Off"
    )]
    pub web: Option<WebConfig>,
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
        locked,
        advanced
    )]
    pub insecure_plaintext: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(group = network, applies = live, advanced)]
    pub heartbeat: Heartbeat,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(group = limits, advanced)]
    pub limits: Limits,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(label = "Pool of idle channels", group = network, applies = reload, advanced)]
    pub pool: Pool,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(label = "TCP", group = network, applies = live, advanced)]
    pub tcp: TcpOptions,
    #[serde(default, skip_serializing_if = "is_default")]
    #[admin(label = "UDP", group = network, applies = live, advanced)]
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
    #[schemars(description = "URL path clients connect to, for example /burrow")]
    #[admin(ty = url_path)]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The name that leads to the tunnel when websocket.listen is the same port as web.https, for example tunnel.example.com"
    )]
    #[admin(
        ty = host_name,
        label = "Name on the HTTPS port",
        help = "The name that leads to the tunnel when it shares the web proxy's HTTPS port, for example tunnel.example.com",
        none_label = "Not shared"
    )]
    pub host: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "How Let's Encrypt checks the tunnel's own name when it shares web.https: tls, http or dns; automatic when not set"
    )]
    #[admin(
        label = "Let's Encrypt check",
        help = "How Let's Encrypt checks the tunnel's own name, for example http behind a proxy that does the HTTPS",
        none_label = "Automatic",
        advanced
    )]
    pub challenge: Option<ChallengeKind>,
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
    #[schemars(
        description = "Certificate chain PEM file, relative to the config file, for example certs/fullchain.pem"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "Certificate file",
        locked
    )]
    pub cert: String,
    #[schemars(
        description = "Private key PEM file, relative to the config file, for example certs/privkey.pem"
    )]
    #[admin(
        ty = path(must_exist, relative_to = "config"),
        label = "Private key file",
        locked
    )]
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
    #[admin(help = "Addresses this server may reach itself, for example 10.0.0.0/8, 192.168.1.10:22 or unix:/run/app.sock; link-local and cloud metadata addresses only when listed one by one", ty = list(cidr_rule), label = "Allowed targets", empty_label = "Nothing")]
    pub allow: Vec<String>,
}
