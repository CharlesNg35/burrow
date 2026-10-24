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
    last: Mutex<Option<Instant>>,
}

impl VisitorStats {
    pub fn seen(&self) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    pub fn last_seen_secs(&self) -> Option<u64> {
        self.last
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|t| t.elapsed().as_secs())
    }
}

pub struct Seen(Arc<VisitorStats>);

impl Seen {
    pub fn new(stats: Arc<VisitorStats>) -> Seen {
        stats.seen();
        Seen(stats)
    }
}

impl Drop for Seen {
    fn drop(&mut self) {
        self.0.seen();
    }
}

const AT_LIMIT_FOR: std::time::Duration = std::time::Duration::from_secs(60);

pub struct Metrics {
    started: Instant,
    services: Mutex<HashMap<String, Arc<ServiceStats>>>,
    visitors: Mutex<HashMap<String, Arc<VisitorStats>>>,
    left: Mutex<HashMap<String, SystemTime>>,
    busy: Mutex<HashMap<String, (u64, Instant)>>,
    pub handshakes_refused: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            started: Instant::now(),
            services: Mutex::default(),
            visitors: Mutex::default(),
            left: Mutex::default(),
            busy: Mutex::default(),
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

    pub fn client_busy(&self, name: &str) {
        let mut busy = self.busy.lock().expect("metrics lock");
        let entry = busy.entry(name.to_string()).or_insert((0, Instant::now()));
        *entry = (entry.0 + 1, Instant::now());
    }

    pub fn busy(&self, name: &str) -> (u64, bool) {
        self.busy
            .lock()
            .expect("metrics lock")
            .get(name)
            .map_or((0, false), |(total, last)| {
                (*total, last.elapsed() < AT_LIMIT_FOR)
            })
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
