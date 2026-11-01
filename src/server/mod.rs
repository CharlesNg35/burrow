pub mod admin_hook;
mod balance;
#[cfg(feature = "web")]
mod certs;
mod control;
mod fields;
mod geo;
mod health;
mod listen;
mod local;
#[cfg(feature = "web")]
mod notify;
pub mod passwords;
mod pick;
mod reload;
mod route;
mod run;
mod session;
mod share;
mod standing;
mod start;
mod status;
mod tcp;
mod udp;
pub mod uploads;
mod via;
mod visit;
mod visit_udp;
#[cfg(feature = "web")]
mod web;
#[cfg(not(feature = "web"))]
mod web_off;
#[cfg(not(feature = "web"))]
use web_off as web;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{Notify, Semaphore, watch};
use tokio::task::JoinHandle;
use tracing::info;

use crate::config::settings::ServerSettings;
use crate::policy::ratelimit::Limit;
pub use session::{ClientSession, PoolConfig, TakeError};
pub use start::bind_error;
#[cfg(feature = "web")]
pub(crate) use start::private_write;
use start::{
    BIND_RETRY, FREED_TRIES, MetricsTask, ServiceTask, Wanted, WsTask, accept_tcp, bind_metrics,
    bind_service, bind_with_retry, bind_ws_with_retry, log_service, spawn_service, take_channel,
};

const TAKE_TIMEOUT: Duration = Duration::from_secs(10);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_secs(5);

pub struct Shared {
    settings: RwLock<Arc<ServerSettings>>,
    sessions: Mutex<HashMap<String, Arc<ClientSession>>>,
    active: AtomicUsize,
    udp_peers: AtomicUsize,
    drained: Notify,
    rejected: AtomicUsize,
    handshakes: RwLock<Arc<Semaphore>>,
    handshake_sources: burrow_syntax::share::Shares<std::net::IpAddr>,
    visitor_shares: share::VisitorShares,
    shutdown: watch::Receiver<bool>,
    reloaded: watch::Sender<u64>,
    applied: watch::Sender<ApplyReport>,
    admin: std::sync::OnceLock<Arc<dyn admin_hook::AdminHook>>,
    service_counts: Counts,
    visitor_counts: Counts,
    client_counts: Counts,
    entry_counts: balance::EntryCounts,
    limits: Mutex<HashMap<String, Arc<Limit>>>,
    probes: Mutex<health::Probes>,
    #[cfg(feature = "web")]
    notifier: notify::Notifier,
    web_wait: Mutex<Option<burrow_admin_proto::status::PortWait>>,
    standing: Mutex<standing::Standing>,
    wheels: Mutex<balance::Wheels>,
    local: Arc<local::SelfDialer>,
    pub(crate) metrics: crate::metrics::Metrics,
    geo: geo::Geo,
    #[cfg(feature = "web")]
    certs: Arc<burrow_web::tls::Store>,
}

pub(crate) struct SessionInfo {
    pub(crate) name: String,
    pub(crate) secret_fp: String,
    pub(crate) from: String,
    pub(crate) allow: Option<Vec<String>>,
    pub(crate) over_websocket: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub seq: u64,
    pub rejected: Option<String>,
    pub unbound: Vec<String>,
    pub restart: Vec<&'static str>,
    pub lines: Vec<String>,
}

#[derive(Default)]
struct Counts(Mutex<HashMap<String, Arc<AtomicUsize>>>);

impl Counts {
    fn enter(&self, name: &str) -> (CountGuard, usize) {
        enter_in(&mut self.0.lock().expect("counts lock"), name)
    }

    fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.0
            .lock()
            .expect("counts lock")
            .retain(|name, c| Arc::strong_count(c) > 1 || keep(name));
    }

    fn current(&self, name: &str) -> u64 {
        current_in(&self.0.lock().expect("counts lock"), name)
    }
}

fn enter_in<K, Q>(counts: &mut HashMap<K, Arc<AtomicUsize>>, key: &Q) -> (CountGuard, usize)
where
    K: std::borrow::Borrow<Q> + Eq + std::hash::Hash,
    Q: Eq + std::hash::Hash + ToOwned<Owned = K> + ?Sized,
{
    let count = crate::base::slots::shared_slot(counts, key, Arc::default);
    let before = count.fetch_add(1, Ordering::AcqRel);
    (CountGuard(count), before)
}

fn current_in<K, Q>(counts: &HashMap<K, Arc<AtomicUsize>>, key: &Q) -> u64
where
    K: std::borrow::Borrow<Q> + Eq + std::hash::Hash,
    Q: Eq + std::hash::Hash + ?Sized,
{
    counts
        .get(key)
        .map_or(0, |c| c.load(Ordering::Acquire) as u64)
}

pub(super) struct CountGuard(Arc<AtomicUsize>);

impl Drop for CountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Shared {
    pub(super) fn refused_handshake(&self) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .handshakes_refused
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn client_opens(&self, client: &str) -> (CountGuard, usize) {
        self.client_counts.enter(client)
    }

    pub(crate) fn allows(&self, client: &str, secret: &crate::wire::secret::Secret) -> bool {
        self.settings()
            .clients
            .get(client)
            .is_some_and(|c| c.secrets.contains(secret))
    }

    pub(crate) fn limit(&self, service: &str) -> Arc<Limit> {
        let rate = self
            .settings()
            .services
            .get(service)
            .and_then(|s| s.bandwidth)
            .unwrap_or(0);
        let mut limits = self.limits.lock().expect("limits lock");
        crate::base::slots::shared_slot(&mut limits, service, || Limit::new(rate))
    }

    fn sync_limits(&self, settings: &ServerSettings) {
        self.wheels
            .lock()
            .expect("wheels lock")
            .retain(&settings.services);
        self.entry_counts.prune(&settings.services);
        self.service_counts
            .retain(|n| settings.services.contains_key(n));
        self.visitor_counts
            .retain(|n| settings.visitors.contains_key(n));
        self.client_counts
            .retain(|n| n == burrow_config::SELF || settings.clients.contains_key(n));
        self.metrics
            .retain(&settings.services, &settings.visitors, &settings.clients);
        let mut limits = self.limits.lock().expect("limits lock");
        limits.retain(|name, _| settings.services.contains_key(name));
        for (name, limit) in limits.iter() {
            limit.set_rate(settings.services[name].bandwidth.unwrap_or(0));
        }
    }

    pub(crate) fn reloads(&self) -> watch::Receiver<u64> {
        self.reloaded.subscribe()
    }

    pub fn applies(&self) -> watch::Receiver<ApplyReport> {
        self.applied.subscribe()
    }

    pub(crate) fn current_settings(&self) -> Arc<ServerSettings> {
        self.settings()
    }

    pub(crate) fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.clone()
    }

    pub(crate) fn client_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .expect("sessions lock")
            .values()
            .map(|s| SessionInfo {
                name: s.name.clone(),
                secret_fp: s.secret.fingerprint(),
                from: s.peer.to_string(),
                allow: s.allow(),
                over_websocket: s.over_websocket,
            })
            .collect()
    }

    pub fn set_admin(&self, hub: Arc<dyn admin_hook::AdminHook>) {
        let _ = self.admin.set(hub);
    }

    pub(crate) fn admin(&self) -> Option<Arc<dyn admin_hook::AdminHook>> {
        self.admin.get().cloned()
    }

    fn report_apply(
        &self,
        rejected: Option<String>,
        unbound: Vec<String>,
        restart: Vec<&'static str>,
        lines: Vec<String>,
    ) {
        self.applied.send_modify(|r| {
            *r = ApplyReport {
                seq: r.seq + 1,
                rejected,
                unbound,
                restart,
                lines,
            }
        });
    }

    fn close_websocket_clients(&self) {
        let moved: Vec<Arc<ClientSession>> = self
            .sessions
            .lock()
            .expect("sessions lock")
            .values()
            .filter(|s| s.over_websocket)
            .cloned()
            .collect();
        for s in moved {
            info!(
                "client \"{}\" came in over the websocket listener that changed; closing its connection so it connects again",
                s.name
            );
            s.close();
        }
    }

    pub fn settings_snapshot(&self) -> ServerSettings {
        (*self.settings()).clone()
    }

    pub fn idle_channels(&self, client: &str) -> usize {
        self.session(client).map_or(0, |s| s.idle_count())
    }

    pub fn session_names(&self) -> Vec<String> {
        self.sessions
            .lock()
            .expect("sessions lock")
            .keys()
            .cloned()
            .collect()
    }

    pub fn active_connections(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    pub fn udp_peers(&self) -> usize {
        self.udp_peers.load(Ordering::Acquire)
    }

    pub fn waiting_visitors(&self) -> usize {
        self.sessions
            .lock()
            .expect("sessions lock")
            .values()
            .map(|s| s.waiter_count())
            .sum()
    }

    fn settings(&self) -> Arc<ServerSettings> {
        self.settings.read().expect("settings lock").clone()
    }

    fn session(&self, name: &str) -> Option<Arc<ClientSession>> {
        self.sessions
            .lock()
            .expect("sessions lock")
            .get(name)
            .cloned()
    }

    fn pool_config(s: &ServerSettings) -> PoolConfig {
        PoolConfig {
            min_idle: s.pool_min_idle,
            max_idle: s.pool_max_idle,
            max_pending: s.max_pending,
            keepalive: s
                .websocket
                .as_ref()
                .map_or(Duration::from_secs(25), |w| w.keepalive),
        }
    }

    fn track(self: &Arc<Self>) -> ActiveConn {
        self.active.fetch_add(1, Ordering::AcqRel);
        ActiveConn {
            shared: self.clone(),
        }
    }
}

struct ActiveConn {
    shared: Arc<Shared>,
}

impl Drop for ActiveConn {
    fn drop(&mut self) {
        if self.shared.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.shared.drained.notify_waiters();
        }
    }
}

pub struct Server {
    shared: Arc<Shared>,
    health: JoinHandle<()>,
    #[cfg(feature = "web")]
    notify: JoinHandle<()>,
    metrics: Option<MetricsTask>,
    geo: Option<geo::GeoTask>,
    listener: Option<TcpListener>,
    websocket: Option<WsTask>,
    web: Option<web::WebTask>,
    services: HashMap<String, ServiceTask>,
    shutdown_tx: watch::Sender<bool>,
    wanted: Wanted,
}

impl Server {
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.listener.as_ref().and_then(|l| l.local_addr().ok())
    }

    pub fn metrics_addr(&self) -> Option<SocketAddr> {
        self.metrics.as_ref().map(|m| m.addr)
    }

    pub fn websocket_addr(&self) -> Option<SocketAddr> {
        self.websocket
            .as_ref()
            .and_then(|w| w.listener.listener.local_addr().ok())
    }

    pub fn service_addr(&self, name: &str) -> Option<SocketAddr> {
        self.services.get(name).and_then(|s| s.addr)
    }

    pub fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use burrow_config::Protocol;

    #[test]
    fn a_count_for_a_name_no_longer_configured_goes_once_nothing_holds_it() {
        let counts = Counts::default();
        let (guard, _) = counts.enter("old");
        counts.enter("kept");
        counts.retain(|n| n == "kept");
        assert_eq!(counts.current("old"), 1, "an open connection keeps it");
        drop(guard);
        counts.retain(|n| n == "kept");
        assert!(!counts.0.lock().unwrap().contains_key("old"));
        assert!(counts.0.lock().unwrap().contains_key("kept"));
    }

    #[test]
    fn busy_port_hint_names_the_platform_tool_and_the_protocol() {
        let busy = std::io::Error::from(std::io::ErrorKind::AddrInUse);
        let addr: SocketAddr = "0.0.0.0:7835".parse().unwrap();
        let tcp = bind_error("the control port", Protocol::Tcp, addr, &busy);
        assert_eq!(
            tcp.message,
            "cannot bind the control port on 0.0.0.0:7835: address already in use"
        );
        let hint = tcp.hint.unwrap();
        assert!(
            hint.ends_with(&crate::base::ports::inspect_hint(Protocol::Tcp, 7835)),
            "{hint}"
        );
        assert_eq!(
            hint.contains("burrow service list"),
            crate::install::any_installed(),
            "{hint}"
        );
        let udp = bind_error("service \"dns\"", Protocol::Udp, addr, &busy)
            .hint
            .unwrap();
        assert!(
            udp.ends_with(&crate::base::ports::inspect_hint(Protocol::Udp, 7835)),
            "{udp}"
        );
        if !cfg!(windows) {
            assert_ne!(hint, udp, "ss, sockstat and lsof take a protocol flag");
        }
    }
}
