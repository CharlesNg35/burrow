pub mod admin_hook;
mod control;
mod fields;
mod health;
mod listen;
mod local;
mod pick;
mod reload;
mod route;
mod run;
mod session;
mod standing;
mod status;
mod tcp;
mod udp;
mod via;
mod visit;
mod visit_udp;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{Notify, Semaphore, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::base::failure::Failure;
use crate::config::settings::WebSocketSettings;
use crate::config::settings::{ServerSettings, Service};
use crate::policy::ratelimit::Limit;
use burrow_config::Protocol;
use listen::WsListener;
pub use session::{ClientSession, PoolConfig, TakeError};

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
    handshake_sources: Arc<Mutex<HashMap<std::net::IpAddr, usize>>>,
    shutdown: watch::Receiver<bool>,
    reloaded: watch::Sender<u64>,
    applied: watch::Sender<ApplyReport>,
    admin: std::sync::OnceLock<Arc<dyn admin_hook::AdminHook>>,
    service_counts: Counts,
    visitor_counts: Counts,
    client_counts: Counts,
    limits: Mutex<HashMap<String, Arc<Limit>>>,
    probes: Mutex<health::Probes>,
    standing: Mutex<standing::Standing>,
    turns: Mutex<HashMap<String, usize>>,
    local: Arc<local::SelfDialer>,
    pub(crate) metrics: crate::metrics::Metrics,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub seq: u64,
    pub rejected: Option<String>,
    pub unbound: Vec<String>,
    pub restart: Vec<&'static str>,
}

#[derive(Default)]
struct Counts(Mutex<HashMap<String, Arc<AtomicUsize>>>);

impl Counts {
    fn enter(&self, name: &str) -> (CountGuard, usize) {
        let count = self
            .0
            .lock()
            .expect("counts lock")
            .entry(name.to_string())
            .or_default()
            .clone();
        let before = count.fetch_add(1, Ordering::AcqRel);
        (CountGuard(count), before)
    }

    fn current(&self, name: &str) -> u64 {
        self.0
            .lock()
            .expect("counts lock")
            .get(name)
            .map_or(0, |c| c.load(Ordering::Acquire) as u64)
    }
}

pub(super) struct CountGuard(Arc<AtomicUsize>);

impl Drop for CountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Shared {
    pub(super) fn client_opens(&self, client: &str) -> (CountGuard, usize) {
        self.client_counts.enter(client)
    }

    pub(crate) fn allows(&self, client: &str, secret: &crate::wire::secret::Secret) -> bool {
        self.settings()
            .clients
            .get(client)
            .is_some_and(|c| c.secrets.contains(secret))
    }

    pub(crate) fn source_allowed(&self, service: &str, ip: std::net::IpAddr) -> bool {
        self.settings()
            .services
            .get(service)
            .is_some_and(|s| s.sources.permits(ip))
    }

    pub(crate) fn limit(&self, service: &str) -> Arc<Limit> {
        let rate = self
            .settings()
            .services
            .get(service)
            .and_then(|s| s.bandwidth)
            .unwrap_or(0);
        self.limits
            .lock()
            .expect("limits lock")
            .entry(service.to_string())
            .or_insert_with(|| Limit::new(rate))
            .clone()
    }

    fn sync_limits(&self, settings: &ServerSettings) {
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

    pub(crate) fn client_sessions(&self) -> Vec<(String, String, String, Option<Vec<String>>)> {
        self.sessions
            .lock()
            .expect("sessions lock")
            .values()
            .map(|s| {
                (
                    s.name.clone(),
                    s.secret.fingerprint(),
                    s.peer.to_string(),
                    s.allow(),
                )
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
    ) {
        self.applied.send_modify(|r| {
            *r = ApplyReport {
                seq: r.seq + 1,
                rejected,
                unbound,
                restart,
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

enum Bound {
    Tcp(TcpListener),
    Udp(UdpSocket),
}

struct ServiceTask {
    service: Service,
    config: watch::Sender<Service>,
    tasks: Vec<JoinHandle<()>>,
    addr: Option<SocketAddr>,
}

impl ServiceTask {
    async fn stop(self) {
        for t in &self.tasks {
            t.abort();
        }
        for t in self.tasks {
            let _ = t.await;
        }
    }
}

pub struct Server {
    shared: Arc<Shared>,
    health: JoinHandle<()>,
    metrics: Option<MetricsTask>,
    listener: Option<TcpListener>,
    websocket: Option<WsTask>,
    services: HashMap<String, ServiceTask>,
    shutdown_tx: watch::Sender<bool>,
    wanted: Wanted,
}

#[derive(Default)]
struct Wanted {
    websocket: Option<WebSocketSettings>,
    metrics: Option<SocketAddr>,
}

struct MetricsTask {
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl MetricsTask {
    fn start(shared: &Arc<Shared>, listener: TcpListener) -> MetricsTask {
        let addr = listener
            .local_addr()
            .expect("a bound listener has an address");
        let snapshot_of = shared.clone();
        let snapshot: crate::metrics::Snapshot = Arc::new(move || snapshot_of.status());
        MetricsTask {
            addr,
            task: tokio::spawn(crate::metrics::serve(listener, snapshot)),
        }
    }
}

fn bind_metrics(addr: SocketAddr) -> Result<TcpListener, Failure> {
    crate::wire::transport::tcp::bind_tcp(addr)
        .map_err(|e| bind_error("the metrics endpoint", Protocol::Tcp, addr, &e))
}

struct WsTask {
    listener: Arc<WsListener>,
    task: JoinHandle<()>,
}

impl WsTask {
    fn start(shared: &Arc<Shared>, listener: WsListener) -> WsTask {
        let listener = Arc::new(listener);
        let task = tokio::spawn(listen::ws_accept_loop(shared.clone(), listener.clone()));
        WsTask { listener, task }
    }
}

async fn accept_tcp(
    listener: &Option<TcpListener>,
) -> std::io::Result<(tokio::net::TcpStream, SocketAddr)> {
    match listener {
        Some(l) => l.accept().await,
        None => std::future::pending().await,
    }
}

async fn take_channel(
    shared: &Shared,
    session: &via::Via,
    label: &str,
    peer: SocketAddr,
) -> Result<crate::wire::transport::Channel, crate::wire::protocol::Status> {
    use crate::wire::protocol::Status;
    match tokio::time::timeout(TAKE_TIMEOUT, session.take()).await {
        Ok(Ok(ch)) => Ok(ch),
        Ok(Err(TakeError::TooManyPending)) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            static WAITING: crate::policy::throttle::Throttle =
                crate::policy::throttle::Throttle::new();
            if let Some(more) = WAITING.allow() {
                warn!(
                    "{label}: too many connections waiting for client \"{}\", dropping one{more}",
                    session.name()
                );
            }
            Err(Status::Busy)
        }
        Ok(Err(TakeError::Closed)) => {
            tracing::debug!(%peer, "{label}: session closed while waiting for a data channel");
            Err(Status::Offline)
        }
        Err(_) => {
            static SLOW: crate::policy::throttle::Throttle =
                crate::policy::throttle::Throttle::new();
            if let Some(more) = SLOW.allow() {
                warn!(
                    "{label}: client \"{}\" did not open a data channel within {}s{more}",
                    session.name(),
                    TAKE_TIMEOUT.as_secs()
                );
            }
            Err(Status::Offline)
        }
    }
}

pub fn bind_error(what: &str, proto: Protocol, addr: SocketAddr, e: &std::io::Error) -> Failure {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => {
            let find = crate::base::ports::inspect_hint(proto, addr.port());
            let hint = if crate::install::any_installed() {
                format!(
                    "a burrow service is installed here and may be using it, see: {}; otherwise find what uses it with: {find}",
                    crate::base::failure::cmd("service list")
                )
            } else {
                format!("pick another port, or find what uses it with: {find}")
            };
            Failure::new(format!(
                "cannot bind {what} on {addr}: address already in use"
            ))
            .hint(hint)
        }
        std::io::ErrorKind::PermissionDenied => Failure::new(format!(
            "cannot bind {what} on {addr}: permission denied"
        ))
        .hint(if cfg!(windows) {
            format!(
                "another program holds this port exclusively, or Windows reserves it; see: netsh interface {} show excludedportrange protocol={}",
                if addr.is_ipv6() { "ipv6" } else { "ipv4" },
                if proto == Protocol::Udp { "udp" } else { "tcp" }
            )
        } else if cfg!(target_os = "macos") {
            "ports below 1024 need root, except on all addresses (0.0.0.0); use a port above 1023".to_string()
        } else if cfg!(target_os = "linux") {
            "ports below 1024 need root or CAP_NET_BIND_SERVICE; use a port above 1023".to_string()
        } else {
            "ports below 1024 need root; use a port above 1023".to_string()
        }),
        std::io::ErrorKind::AddrNotAvailable => Failure::new(format!(
            "cannot bind {what} on {addr}: address not available on this machine"
        ))
        .hint(format!(
            "use 0.0.0.0 or an IP address assigned to this machine (see: {})",
            crate::base::failure::addresses_cmd()
        )),
        _ => Failure::new(format!(
            "cannot bind {what} on {addr}: {}",
            crate::base::failure::io_reason(e)
        )),
    }
}

async fn bind_service(name: &str, svc: &Service) -> Result<Vec<(u16, Bound)>, Failure> {
    let what = format!("service \"{name}\"");
    let mut bound = Vec::new();
    for (index, addr) in svc.binds() {
        let b = match svc.protocol {
            Protocol::Tcp => crate::wire::transport::tcp::bind_tcp(addr)
                .map(Bound::Tcp)
                .map_err(|e| bind_error(&what, Protocol::Tcp, addr, &e))?,
            Protocol::Udp => crate::wire::transport::udp::bind_udp(addr)
                .await
                .map(Bound::Udp)
                .map_err(|e| bind_error(&what, Protocol::Udp, addr, &e))?,
        };
        bound.push((index, b));
    }
    Ok(bound)
}

impl Server {
    pub async fn bind(settings: ServerSettings) -> Result<Server, Failure> {
        let listener = match settings.listen {
            Some(addr) => Some(
                crate::wire::transport::tcp::bind_tcp(addr)
                    .map_err(|e| bind_error("the control port", Protocol::Tcp, addr, &e))?,
            ),
            None => None,
        };
        let ws_listener = match &settings.websocket {
            Some(ws) => Some(WsListener::bind(ws)?),
            None => None,
        };
        let metrics_listener = settings.metrics.map(bind_metrics).transpose()?;
        let (shutdown_tx, shutdown) = watch::channel(false);
        let shared = Arc::new(Shared {
            settings: RwLock::new(Arc::new(settings.clone())),
            sessions: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
            udp_peers: AtomicUsize::new(0),
            drained: Notify::new(),
            rejected: AtomicUsize::new(0),
            handshakes: RwLock::new(Arc::new(Semaphore::new(settings.max_handshakes))),
            handshake_sources: Arc::default(),
            shutdown,
            reloaded: watch::Sender::new(0),
            applied: watch::Sender::new(ApplyReport::default()),
            admin: std::sync::OnceLock::new(),
            service_counts: Counts::default(),
            visitor_counts: Counts::default(),
            client_counts: Counts::default(),
            limits: Mutex::new(HashMap::new()),
            probes: Mutex::new(HashMap::new()),
            standing: Mutex::new(standing::Standing::default()),
            turns: Mutex::new(HashMap::new()),
            local: Arc::new(local::SelfDialer::new(
                &settings,
                Vec::new(),
                crate::wire::transport::resolve::system(),
            )),
            metrics: crate::metrics::Metrics::default(),
        });
        let mut services = HashMap::new();
        for (name, svc) in &settings.services {
            if svc.bind.is_none() {
                continue;
            }
            let bound = bind_service(name, svc).await?;
            services.insert(name.clone(), spawn_service(&shared, name, svc, bound));
        }
        let websocket = ws_listener.map(|l| WsTask::start(&shared, l));
        let health = tokio::spawn(health::run(shared.clone()));
        let metrics = metrics_listener.map(|l| MetricsTask::start(&shared, l));
        let server = Server {
            health,
            metrics,
            shared,
            listener,
            websocket,
            services,
            shutdown_tx,
            wanted: Wanted::default(),
        };
        server.refresh_self();
        Ok(server)
    }

    fn own_addrs(&self) -> Vec<SocketAddr> {
        [
            self.local_addr(),
            self.websocket_addr(),
            self.metrics_addr(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn refresh_self(&self) {
        self.shared
            .local
            .reconfigure(&self.shared.settings(), self.own_addrs());
    }

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

async fn bind_ws_with_retry(
    ws: &crate::config::settings::WebSocketSettings,
    tries: usize,
) -> Result<WsListener, Failure> {
    let mut last = None;
    for _ in 0..tries {
        match WsListener::bind(ws) {
            Ok(l) => return Ok(l),
            Err(f) => last = Some(f),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(last.expect("at least one attempt"))
}

const FREED_TRIES: usize = 20;
const BIND_RETRY: Duration = Duration::from_secs(10);

async fn bind_with_retry(
    name: &str,
    svc: &Service,
    tries: usize,
) -> Result<Vec<(u16, Bound)>, Failure> {
    let mut last = None;
    for _ in 0..tries {
        match bind_service(name, svc).await {
            Ok(b) => return Ok(b),
            Err(f) => last = Some(f),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(last.expect("at least one attempt"))
}

fn log_service(name: &str, svc: &Service, addr: Option<SocketAddr>) {
    let at = addr.or(svc.bind).map_or_else(
        || "private".to_string(),
        |a| crate::config::check::ports::show_addr(a, svc.ports),
    );
    let mut extras = Vec::new();
    if !svc.visitors.is_empty() {
        extras.push(format!("visitors: {}", svc.visitors.join(", ")));
    }
    if let Some(rate) = svc.bandwidth {
        extras.push(format!(
            "limited to {}",
            crate::policy::ratelimit::describe(rate)
        ));
    }
    let clients = match svc.clients.as_slice() {
        [one] => format!("client \"{one}\""),
        many => format!(
            "clients {} ({})",
            many.iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", "),
            match svc.mode {
                burrow_config::Mode::Failover => "failover",
                burrow_config::Mode::Balance => "balance",
            }
        ),
    };
    if svc.health.is_some() {
        extras.push("health checked".into());
    }
    let visitors = if extras.is_empty() {
        String::new()
    } else {
        format!(" ({})", extras.join("; "))
    };
    info!(
        "service \"{name}\": {} {at} -> {clients} {}{visitors}",
        svc.protocol.wire(),
        svc.shown_target()
    );
}

fn spawn_service(
    shared: &Arc<Shared>,
    name: &str,
    svc: &Service,
    bound: Vec<(u16, Bound)>,
) -> ServiceTask {
    let (config, rx) = watch::channel(svc.clone());
    let mut addr = None;
    let mut tasks = Vec::with_capacity(bound.len());
    for (index, b) in bound {
        let (shared, name, rx) = (shared.clone(), name.to_string(), rx.clone());
        let (at, task) = match b {
            Bound::Tcp(l) => (
                l.local_addr().ok(),
                tokio::spawn(tcp::tcp_service(shared, name, l, rx, index)),
            ),
            Bound::Udp(sock) => (
                sock.local_addr().ok(),
                tokio::spawn(udp::udp_service(shared, name, sock, rx, index)),
            ),
        };
        addr = addr.or(at);
        tasks.push(task);
    }
    ServiceTask {
        service: svc.clone(),
        config,
        tasks,
        addr,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

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
