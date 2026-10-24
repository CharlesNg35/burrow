use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

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
        help = "Connections this server forwards to one client at once, unless that client has its own Max connections on this server",
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
    #[admin(
        ty = integer(unit = "channels"),
        label = "Max idle channels",
        help = "Most data channels each client keeps ready; the pool grows toward it when busy and shrinks when idle"
    )]
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
