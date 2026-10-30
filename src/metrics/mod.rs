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
    pub http: [AtomicU64; 5],
}

impl ServiceStats {
    pub fn answered(&self, status: u16) {
        let class = usize::from(status / 100).clamp(1, 5) - 1;
        self.http[class].fetch_add(1, Ordering::Relaxed);
    }

    pub fn http_counts(&self) -> [u64; 5] {
        std::array::from_fn(|i| self.http[i].load(Ordering::Relaxed))
    }
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
        let mut all = self.services.lock().expect("metrics lock");
        crate::base::slots::shared_slot(&mut all, name, Arc::default)
    }

    pub fn visitor(&self, name: &str) -> Arc<VisitorStats> {
        let mut all = self.visitors.lock().expect("metrics lock");
        crate::base::slots::shared_slot(&mut all, name, Arc::default)
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
        match busy.get_mut(name) {
            Some(entry) => *entry = (entry.0 + 1, Instant::now()),
            None => {
                busy.insert(name.to_string(), (1, Instant::now()));
            }
        }
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

    pub fn retain<S, V, C>(
        &self,
        services: &std::collections::BTreeMap<String, S>,
        visitors: &std::collections::BTreeMap<String, V>,
        clients: &std::collections::BTreeMap<String, C>,
    ) {
        self.services
            .lock()
            .expect("metrics lock")
            .retain(|n, s| Arc::strong_count(s) > 1 || services.contains_key(n));
        self.visitors
            .lock()
            .expect("metrics lock")
            .retain(|n, s| Arc::strong_count(s) > 1 || visitors.contains_key(n));
        self.left
            .lock()
            .expect("metrics lock")
            .retain(|n, _| n == burrow_config::SELF || clients.contains_key(n));
        self.busy
            .lock()
            .expect("metrics lock")
            .retain(|n, _| n == burrow_config::SELF || clients.contains_key(n));
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
    AcmeStatus, CertSourceStatus, CertStateStatus, CertStatus, ClientStatus, HealthStatus,
    LocalCaStatus, ServiceStatus, SetAside, Status, VisitorStatus,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_no_longer_configured_are_forgotten_unless_in_use() {
        let m = Metrics::default();
        let held = m.service("old");
        m.service("gone").total.fetch_add(1, Ordering::Relaxed);
        m.visitor("ops");
        m.visitor("former");
        m.client_left("home");
        m.client_left("before");
        m.client_busy("before");
        m.client_busy(burrow_config::SELF);
        let named = |names: &[&str]| -> std::collections::BTreeMap<String, ()> {
            names.iter().map(|n| (n.to_string(), ())).collect()
        };
        m.retain(&named(&[]), &named(&["ops"]), &named(&["home"]));
        let services = m.services.lock().unwrap();
        assert!(services.contains_key("old") && !services.contains_key("gone"));
        drop(services);
        let visitors = m.visitors.lock().unwrap();
        assert!(visitors.contains_key("ops") && !visitors.contains_key("former"));
        drop(visitors);
        assert!(m.left_secs_ago("home").is_some());
        assert_eq!(m.left_secs_ago("before"), None);
        assert_eq!(m.busy("before"), (0, false));
        assert_eq!(m.busy(burrow_config::SELF).0, 1);
        drop(held);
        m.retain(&named(&[]), &named(&["ops"]), &named(&["home"]));
        assert!(m.services.lock().unwrap().is_empty());
    }

    #[test]
    fn web_responses_are_counted_by_status_class() {
        let s = ServiceStats::default();
        for status in [200, 204, 308, 404, 404, 503, 101, 999] {
            s.answered(status);
        }
        assert_eq!(s.http_counts(), [1, 2, 1, 2, 2]);
    }
}
