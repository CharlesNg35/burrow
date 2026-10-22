use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub uptime_secs: u64,
    #[serde(default)]
    pub listening: Vec<String>,
    #[serde(default)]
    pub handshakes_refused: u64,
    #[serde(default)]
    pub udp_peers: u64,
    #[serde(default)]
    pub clients: Vec<ClientStatus>,
    #[serde(default)]
    pub services: Vec<ServiceStatus>,
    #[serde(default)]
    pub visitors: Vec<VisitorStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClientStatus {
    pub name: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub connected_secs: Option<u64>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub open: u64,
    #[serde(default)]
    pub idle_channels: u64,
    #[serde(default)]
    pub left_secs_ago: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    #[serde(default)]
    pub protocol: String,
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub clients: Vec<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub bandwidth: Option<u64>,
    #[serde(default)]
    pub visitors: Vec<String>,
    #[serde(default)]
    pub proxy_protocol: Option<String>,
    #[serde(default)]
    pub filtered: bool,
    #[serde(default)]
    pub open: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub refused: u64,
    #[serde(default)]
    pub bytes_in: u64,
    #[serde(default)]
    pub bytes_out: u64,
    #[serde(default)]
    pub health: Vec<HealthStatus>,
    #[serde(default)]
    pub set_aside: Vec<SetAside>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SetAside {
    pub client: String,
    #[serde(default)]
    pub refused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HealthStatus {
    pub client: String,
    #[serde(default)]
    pub up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VisitorStatus {
    pub name: String,
    #[serde(default)]
    pub open: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub last_seen_secs: Option<u64>,
}
