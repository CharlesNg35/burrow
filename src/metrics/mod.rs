mod counted;
pub mod render;
mod serve;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};

pub use counted::Counted;
pub use render::prometheus;
pub use serve::{Handler, Snapshot, serve, serve_with};

#[derive(Debug, Default)]
pub struct Traffic {
    pub inbound: AtomicU64,
    pub outbound: AtomicU64,
}

#[derive(Debug, Default)]
pub struct ServiceStats {
    pub total: AtomicU64,
    pub refused: AtomicU64,
    pub traffic: Traffic,
}

#[derive(Debug, Default)]
pub struct VisitorStats {
    pub total: AtomicU64,
}

pub struct Metrics {
    started: Instant,
    services: Mutex<HashMap<String, Arc<ServiceStats>>>,
    visitors: Mutex<HashMap<String, Arc<VisitorStats>>>,
    left: Mutex<HashMap<String, SystemTime>>,
    pub handshakes_refused: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            started: Instant::now(),
            services: Mutex::default(),
            visitors: Mutex::default(),
            left: Mutex::default(),
            handshakes_refused: AtomicU64::new(0),
        }
    }
}

impl Metrics {
    pub fn service(&self, name: &str) -> Arc<ServiceStats> {
        self.services
            .lock()
            .expect("metrics lock")
            .entry(name.to_string())
            .or_default()
            .clone()
    }

    pub fn visitor(&self, name: &str) -> Arc<VisitorStats> {
        self.visitors
            .lock()
            .expect("metrics lock")
            .entry(name.to_string())
            .or_default()
            .clone()
    }

    pub fn refuse(&self, service: &str) {
        let stats = self.service(service);
        stats.total.fetch_add(1, Ordering::Relaxed);
        stats.refused.fetch_add(1, Ordering::Relaxed);
    }

    pub fn client_left(&self, name: &str) {
        self.left
            .lock()
            .expect("metrics lock")
            .insert(name.to_string(), SystemTime::now());
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    pub fn left_secs_ago(&self, name: &str) -> Option<u64> {
        self.left
            .lock()
            .expect("metrics lock")
            .get(name)
            .and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub uptime_secs: u64,
    #[serde(default)]
    pub listening: Vec<String>,
    pub handshakes_refused: u64,
    pub udp_peers: u64,
    pub clients: Vec<ClientStatus>,
    pub services: Vec<ServiceStatus>,
    pub visitors: Vec<VisitorStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientStatus {
    pub name: String,
    pub connected: bool,
    pub connected_secs: Option<u64>,
    pub from: Option<String>,
    pub open: u64,
    pub idle_channels: u64,
    pub left_secs_ago: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    pub protocol: String,
    pub bind: Option<String>,
    pub target: String,
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
    pub open: u64,
    pub total: u64,
    pub refused: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub health: Vec<HealthStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthStatus {
    pub client: String,
    pub up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitorStatus {
    pub name: String,
    pub open: u64,
    pub total: u64,
}

pub fn fetch(addr: std::net::SocketAddr) -> Result<Status, String> {
    use std::io::{Read, Write};
    let timeout = std::time::Duration::from_secs(5);
    let mut s = std::net::TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    s.write_all(
        format!("GET /status HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.take(16 * 1024 * 1024)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("the answer is not HTTP")?;
    if !raw.starts_with(b"HTTP/1.1 200") {
        return Err("the metrics endpoint did not answer 200".into());
    }
    serde_json::from_slice(&raw[split + 4..]).map_err(|e| e.to_string())
}
