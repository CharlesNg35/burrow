use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

use super::client::Cipher;
use super::client::{default_connect_timeout, is_default_connect_timeout};
use super::entries::{Bind, Protocol, Target};
use super::options::{TcpOptions, UdpOptions};
use super::{is_default, is_false};

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
