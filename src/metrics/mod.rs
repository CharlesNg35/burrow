mod counted;
pub mod render;
mod serve;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

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

pub use burrow_admin_proto::status::{
    ClientStatus, HealthStatus, ServiceStatus, SetAside, Status, VisitorStatus,
};
