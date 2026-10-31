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
        label = "Connection setup timeout",
        help = "How long a new client or visitor connection may take to show its key before it is closed",
        applies = live,
        locked
    )]
    pub handshake_timeout_secs: u32,
    #[schemars(description = "Concurrent connections that have not finished authenticating")]
    #[schemars(range(min = 1, max = 1000000))]
    #[admin(
        ty = integer(unit = "connections"),
        label = "Maximum connections setting up",
        help = "New client and visitor connections that may show their key at the same time; more wait",
        applies = reload,
        locked
    )]
    pub max_handshakes: u32,
    #[schemars(range(min = 1))]
    #[admin(
        ty = integer(unit = "connections"),
        help = "Connections this server forwards to one client at once, unless that client has its own Maximum connections on this server",
        applies = live
    )]
    pub max_connections_per_client: u32,
    #[schemars(description = "Connections waiting for a data channel, per client")]
    #[schemars(range(min = 1))]
    #[admin(ty = integer(unit = "connections"), label = "Maximum waiting connections", applies = reload)]
    pub max_pending: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            handshake_timeout_secs: 5,
            max_handshakes: 2048,
            max_connections_per_client: 8192,
            max_pending: 512,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, AdminSchema)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, default)]
#[admin(label = "Ready connections")]
pub struct Pool {
    #[schemars(range(max = 1024))]
    #[admin(
        ty = integer(unit = "connections"),
        label = "Kept ready",
        help = "Spare connections each client keeps open, so a new visitor connection starts without waiting"
    )]
    pub min_idle: u32,
    #[schemars(range(min = 1, max = 4096))]
    #[admin(
        ty = integer(unit = "connections"),
        label = "Maximum kept ready",
        help = "Most spare connections each client keeps; more are added when busy and dropped when idle"
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
        label = "Send small packets at once",
        help = "Send small writes at once instead of batching them (TCP_NODELAY)"
    )]
    pub nodelay: bool,
    #[schemars(
        description = "TCP keepalive idle time, 0 disables",
        range(max = 32767)
    )]
    #[admin(
        help = "Idle time before burrow checks that a quiet connection is still there (TCP keepalive); 0 turns the checks off",
        label = "Check idle connections"
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
        help = "A UDP sender that sends nothing for this long is forgotten"
    )]
    pub session_timeout_secs: u32,
    #[schemars(
        description = "Most UDP peers one service port keeps at once; a datagram from a new peer beyond it is dropped",
        range(min = 1, max = 1000000),
        extend("x-show-default" = true)
    )]
    #[admin(
        ty = integer(unit = "senders"),
        label = "Maximum UDP senders per port",
        help = "Most UDP senders one service port keeps at once; a datagram from a new sender beyond it is dropped. For example 20000; the default is 4096",
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
