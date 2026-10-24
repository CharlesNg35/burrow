use std::collections::BTreeMap;

use burrow_admin_proto::msg::AdminLevel;
use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

use super::is_default;

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
    #[admin(help = "Clients this visitor may reach directly, over TCP and UDP: any target the client allows, or only those listed, for example 10.0.0.5:22 or 192.168.150.0/25",
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
    #[admin(
        help = "TCP for most services; UDP for DNS, WireGuard or game servers",
        group = exposure
    )]
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
