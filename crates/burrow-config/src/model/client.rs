use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::admin_schema::AdminSchema;

use super::options::{ClientUdpOptions, TcpOptions};
use super::{is_default, is_false};

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

pub(super) fn default_connect_timeout() -> u32 {
    10
}

pub(super) fn is_default_connect_timeout(v: &u32) -> bool {
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
