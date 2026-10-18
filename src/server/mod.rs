mod control;
mod fields;
mod health;
mod listen;
mod pick;
mod route;
mod session;
mod standing;
mod status;
mod tcp;
mod udp;
mod visit;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{Notify, Semaphore, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::config::model::Protocol;
use crate::config::settings::WebSocketSettings;
use crate::config::settings::{ServerSettings, Service};
use crate::failure::Failure;
use crate::protocol::{ByeReason, ServerMsg};
use crate::ratelimit::Limit;
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
    shutdown: watch::Receiver<bool>,
    reloaded: watch::Sender<u64>,
    service_counts: Counts,
    visitor_counts: Counts,
    client_counts: Counts,
    limits: Mutex<HashMap<String, Arc<Limit>>>,
    probes: Mutex<health::Probes>,
    standing: Mutex<standing::Standing>,
    turns: Mutex<HashMap<String, usize>>,
    pub(crate) metrics: crate::metrics::Metrics,
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

    pub(crate) fn allows(&self, client: &str, secret: &crate::secret::Secret) -> bool {
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
    crate::transport::tcp::bind_tcp(addr)
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
    session: &ClientSession,
    label: &str,
    peer: SocketAddr,
) -> Result<crate::handshake::Channel, crate::protocol::Status> {
    use crate::protocol::Status;
    match tokio::time::timeout(TAKE_TIMEOUT, session.take()).await {
        Ok(Ok(ch)) => Ok(ch),
        Ok(Err(TakeError::TooManyPending)) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            static WAITING: crate::throttle::Throttle = crate::throttle::Throttle::new();
            if let Some(more) = WAITING.allow() {
                warn!(
                    "{label}: too many connections waiting for client \"{}\", dropping one{more}",
                    session.name
                );
            }
            Err(Status::Busy)
        }
        Ok(Err(TakeError::Closed)) => {
            tracing::debug!(%peer, "{label}: session closed while waiting for a data channel");
            Err(Status::Offline)
        }
        Err(_) => {
            static SLOW: crate::throttle::Throttle = crate::throttle::Throttle::new();
            if let Some(more) = SLOW.allow() {
                warn!(
                    "{label}: client \"{}\" did not open a data channel within {}s{more}",
                    session.name,
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
            let find = crate::service::ports::inspect_hint(proto, addr.port());
            let hint = if crate::service::any_installed() {
                format!(
                    "a burrow service is installed here and may be using it, see: {}; otherwise find what uses it with: {find}",
                    crate::failure::cmd("service list")
                )
            } else {
                format!("pick another port, or find what uses it with: {find}")
            };
            Failure::new(format!(
                "cannot bind {what} on {addr}: address already in use"
            ))
            .hint(hint)
        }
        std::io::ErrorKind::PermissionDenied => {
            Failure::new(format!("cannot bind {what} on {addr}: permission denied"))
                .hint("ports below 1024 need root or CAP_NET_BIND_SERVICE; use a port above 1024")
        }
        std::io::ErrorKind::AddrNotAvailable => Failure::new(format!(
            "cannot bind {what} on {addr}: address not available on this machine"
        ))
        .hint("use 0.0.0.0 or an IP address assigned to this machine (see: ip addr)"),
        _ => Failure::new(format!(
            "cannot bind {what} on {addr}: {}",
            crate::failure::io_reason(e)
        )),
    }
}

async fn bind_service(name: &str, svc: &Service) -> Result<Vec<(u16, Bound)>, Failure> {
    let what = format!("service \"{name}\"");
    let mut bound = Vec::new();
    for (index, addr) in svc.binds() {
        let b = match svc.protocol {
            Protocol::Tcp => crate::transport::tcp::bind_tcp(addr)
                .map(Bound::Tcp)
                .map_err(|e| bind_error(&what, Protocol::Tcp, addr, &e))?,
            Protocol::Udp => crate::transport::udp::bind_udp(addr)
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
                crate::transport::tcp::bind_tcp(addr)
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
            shutdown,
            reloaded: watch::Sender::new(0),
            service_counts: Counts::default(),
            visitor_counts: Counts::default(),
            client_counts: Counts::default(),
            limits: Mutex::new(HashMap::new()),
            probes: Mutex::new(HashMap::new()),
            standing: Mutex::new(standing::Standing::default()),
            turns: Mutex::new(HashMap::new()),
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
        Ok(Server {
            health,
            metrics,
            shared,
            listener,
            websocket,
            services,
            shutdown_tx,
        })
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

    pub async fn run(
        mut self,
        mut reload: mpsc::Receiver<ServerSettings>,
        mut stop: watch::Receiver<bool>,
    ) {
        let s = self.shared.settings();
        if s.insecure_plaintext {
            warn!("insecure_plaintext is enabled: forwarded TCP data is NOT encrypted");
        }
        if let Some(addr) = self.local_addr() {
            info!("listening for clients on {addr}");
        }
        if let Some(ws) = &self.websocket {
            info!(
                "listening for WebSocket clients on {}",
                ws.listener.url_hint()
            );
        }
        if let Some(m) = &self.metrics {
            info!("metrics on http://{}/metrics", m.addr);
        }
        for (name, svc) in &s.services {
            log_service(name, svc, self.services.get(name).and_then(|t| t.addr));
        }
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.tick().await;
        let mut retry = tokio::time::interval(BIND_RETRY);
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        retry.tick().await;
        loop {
            tokio::select! {
                r = accept_tcp(&self.listener) => match r {
                    Ok((stream, peer)) => {
                        let Some(permit) = listen::try_permit(&self.shared, peer) else {
                            continue;
                        };
                        let shared = self.shared.clone();
                        tokio::spawn(async move {
                            listen::handle_tcp(shared, stream, peer, permit).await;
                        });
                    }
                    Err(e) => {
                        static FAILED: crate::throttle::Throttle = crate::throttle::Throttle::new();
                        if let Some(more) = FAILED.allow() {
                            warn!("accept failed: {e}{more}");
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
                Some(next) = reload.recv() => self.apply(next).await,
                _ = retry.tick() => self.bind_waiting().await,
                _ = tick.tick() => {
                    let n = self.shared.rejected.swap(0, Ordering::Relaxed);
                    if n > 0 {
                        warn!("rejected {n} connections in the last minute (failed handshakes or over limits)");
                    }
                }
                _ = stop.changed() => break,
            }
        }
        self.shutdown().await;
    }

    async fn bind_waiting(&mut self) {
        let settings = self.shared.settings();
        for (name, svc) in &settings.services {
            if svc.bind.is_none() || self.services.contains_key(name) {
                continue;
            }
            if let Ok(bound) = bind_service(name, svc).await {
                let t = spawn_service(&self.shared, name, svc, bound);
                info!("service \"{name}\" could listen now");
                log_service(name, svc, t.addr);
                self.services.insert(name.clone(), t);
            }
        }
    }

    async fn shutdown(mut self) {
        info!("shutting down");
        let _ = self.shutdown_tx.send(true);
        self.health.abort();
        if let Some(m) = self.metrics.take() {
            m.task.abort();
        }
        if let Some(ws) = self.websocket.take() {
            ws.task.abort();
        }
        for (_, t) in self.services.drain() {
            for task in &t.tasks {
                task.abort();
            }
        }
        let sessions: Vec<_> = self
            .shared
            .sessions
            .lock()
            .expect("sessions lock")
            .drain()
            .map(|(_, s)| s)
            .collect();
        for s in sessions {
            let _ = s.control.try_send(ServerMsg::Bye {
                reason: ByeReason::Shutdown,
            });
            s.close();
        }
        let active = self.shared.active.load(Ordering::Acquire);
        if active > 0 {
            info!(
                "waiting up to {}s for {active} active connections",
                DRAIN_TIMEOUT.as_secs()
            );
            let wait = async {
                while self.shared.active.load(Ordering::Acquire) > 0 {
                    let n = self.shared.drained.notified();
                    if self.shared.active.load(Ordering::Acquire) == 0 {
                        break;
                    }
                    n.await;
                }
            };
            let _ = tokio::time::timeout(DRAIN_TIMEOUT, wait).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    async fn apply(&mut self, next: ServerSettings) {
        let current = self.shared.settings();
        if next.listen != current.listen {
            let clash = current.listen.and_then(|kept| {
                let conflicts = |a| crate::config::check::binds_conflict(kept, a);
                next.services
                    .iter()
                    .find(|(_, s)| {
                        s.protocol == Protocol::Tcp
                            && s.binds().into_iter().any(|(_, a)| conflicts(a))
                    })
                    .map(|(n, _)| format!("service \"{n}\""))
                    .or_else(|| {
                        next.websocket
                            .as_ref()
                            .filter(|w| conflicts(w.listen))
                            .map(|_| "websocket.listen".to_string())
                    })
                    .map(|what| (what, kept))
            });
            if let Some((what, kept)) = clash {
                error!(
                    "reload rejected: {what} uses {kept}, which this server keeps listening on until it restarts; keeping the current config"
                );
                return;
            }
            let show =
                |a: Option<SocketAddr>| a.map_or_else(|| "off".to_string(), |a| a.to_string());
            warn!(
                "changing \"listen\" from {} to {} needs a restart; keeping {}",
                show(current.listen),
                show(next.listen),
                show(current.listen)
            );
        }
        let mut next = next;
        next.listen = current.listen;
        let changed = fields::changed(&current, &next);
        for (name, _) in changed
            .iter()
            .filter(|(_, how)| *how == fields::Applied::Restart)
        {
            warn!("\"{name}\" takes effect when the server restarts");
        }
        if next.max_handshakes != current.max_handshakes {
            *self.shared.handshakes.write().expect("handshakes lock") =
                Arc::new(Semaphore::new(next.max_handshakes));
        }
        *self.shared.settings.write().expect("settings lock") = Arc::new(next.clone());
        self.shared.sync_limits(&next);
        self.shared.prune_standing(&next);
        self.shared.reloaded.send_modify(|n| *n += 1);

        if next.websocket != current.websocket {
            let kept = self
                .apply_websocket(current.websocket.as_ref(), next.websocket.as_ref())
                .await;
            if kept != next.websocket {
                next.websocket = kept;
                *self.shared.settings.write().expect("settings lock") = Arc::new(next.clone());
            }
        }
        if next.metrics != current.metrics {
            self.apply_metrics(next.metrics).await;
        }

        let moving: Vec<String> = self
            .services
            .iter()
            .filter(|(k, t)| {
                next.services.get(*k).is_none_or(|s| {
                    s.bind != t.service.bind
                        || s.protocol != t.service.protocol
                        || s.ports != t.service.ports
                })
            })
            .map(|(k, _)| k.clone())
            .collect();
        let mut released: Vec<(Protocol, SocketAddr, u16)> = Vec::new();
        let mut unbound: Vec<String> = Vec::new();
        for name in moving {
            if let Some(t) = self.services.remove(&name) {
                if let Some(b) = t.service.bind {
                    released.push((t.service.protocol, b, t.service.ports));
                }
                t.stop().await;
                match next.services.get(&name) {
                    None => info!("service \"{name}\" removed"),
                    Some(s) if s.bind.is_none() => {
                        info!("service \"{name}\" is now private");
                        log_service(&name, s, None);
                    }
                    Some(_) => {}
                }
            }
        }
        for (name, svc) in &next.services {
            if svc.bind.is_none() {
                if current
                    .services
                    .get(name)
                    .is_some_and(|c| c.bind.is_none() && c != svc)
                {
                    info!("service \"{name}\" updated");
                    log_service(name, svc, None);
                } else if !current.services.contains_key(name) {
                    info!("service \"{name}\" added");
                    log_service(name, svc, None);
                }
                continue;
            }
            let same_socket = self.services.get(name).is_some_and(|t| {
                t.service.bind == svc.bind
                    && t.service.protocol == svc.protocol
                    && t.service.ports == svc.ports
            });
            if same_socket {
                let t = self.services.get_mut(name).expect("checked above");
                if t.service != *svc {
                    t.service = svc.clone();
                    let _ = t.config.send(svc.clone());
                    info!("service \"{name}\" updated");
                    log_service(name, svc, t.addr);
                }
                continue;
            }
            if let Some(old) = self.services.remove(name) {
                if let Some(b) = old.service.bind {
                    released.push((old.service.protocol, b, old.service.ports));
                }
                old.stop().await;
            }
            let freed = svc.bind.is_some_and(|b| {
                released.iter().any(|(p, a, n)| {
                    *p == svc.protocol
                        && crate::config::check::ports::ranges_conflict(*a, *n, b, svc.ports)
                })
            });
            let tries = if freed { FREED_TRIES } else { 1 };
            match bind_with_retry(name, svc, tries).await {
                Ok(bound) => {
                    let t = spawn_service(&self.shared, name, svc, bound);
                    if !current.services.get(name).is_some_and(|c| c.bind.is_none()) {
                        info!("service \"{name}\" added");
                    }
                    log_service(name, svc, t.addr);
                    self.services.insert(name.clone(), t);
                }
                Err(f) => {
                    error!("{}", f.message);
                    unbound.push(format!("\"{name}\""));
                }
            }
        }

        for (name, svc) in &current.services {
            match next.services.get(name) {
                None if svc.bind.is_none() => info!("service \"{name}\" removed"),
                Some(n) if svc.bind.is_none() && n.bind.is_some() => {
                    info!("service \"{name}\" is now public")
                }
                _ => {}
            }
        }

        let sessions: Vec<Arc<ClientSession>> = self
            .shared
            .sessions
            .lock()
            .expect("sessions lock")
            .values()
            .cloned()
            .collect();
        let toggled = next.insecure_plaintext != current.insecure_plaintext;
        for s in sessions {
            let why = match next.clients.get(&s.name) {
                None => Some(("was removed from the config", ByeReason::Removed)),
                Some(c) if !c.secrets.contains(&s.secret) => {
                    Some(("lost its secret", ByeReason::Removed))
                }
                Some(_) if toggled => Some((
                    "has to reconnect, because insecure_plaintext changed",
                    ByeReason::PlaintextMismatch,
                )),
                Some(_) => None,
            };
            if let Some((why, reason)) = why {
                info!(
                    "client \"{}\" {why}; disconnecting it and its open connections",
                    s.name
                );
                self.shared.metrics.client_left(&s.name);
                let _ = s.control.try_send(ServerMsg::Bye { reason });
                s.close();
                self.shared
                    .sessions
                    .lock()
                    .expect("sessions lock")
                    .retain(|_, v| !Arc::ptr_eq(v, &s));
                continue;
            }
            s.reconfigure(&Shared::pool_config(&next));
            let _ = s
                .control
                .try_send(ServerMsg::Services(next.services_for(&s.name)));
        }
        let (c, v) = (next.clients.len(), next.services.len());
        let names: Vec<&str> = changed.iter().map(|(name, _)| *name).collect();
        let what = if names.is_empty() {
            String::new()
        } else {
            format!("; changed: {}", names.join(", "))
        };
        let summary = format!(
            "config reloaded ({c} {}, {v} {}{what})",
            crate::diagnostics::plural(c, "client"),
            crate::diagnostics::plural(v, "service")
        );
        if unbound.is_empty() {
            info!("{summary}");
        } else {
            warn!(
                "{summary}, but service {} could not listen; trying again every {}s",
                unbound.join(", "),
                BIND_RETRY.as_secs()
            );
        }
    }
}

impl Server {
    async fn apply_websocket(
        &mut self,
        current: Option<&WebSocketSettings>,
        next: Option<&WebSocketSettings>,
    ) -> Option<WebSocketSettings> {
        let same_socket = self.websocket.is_some()
            && matches!((current, next), (Some(c), Some(n)) if c.listen == n.listen);
        match (next, same_socket) {
            (Some(n), true) => match listen::WsRoute::new(n) {
                Ok(route) => {
                    let t = self.websocket.as_ref().expect("checked above");
                    t.listener.set_route(route);
                    let only_tls = current.is_some_and(|c| {
                        c.listen == n.listen && c.path == n.path && c.keepalive == n.keepalive
                    });
                    if only_tls {
                        info!("websocket TLS certificate reloaded; new connections use it");
                    } else {
                        info!("websocket listener updated: {}", t.listener.url_hint());
                    }
                    Some(n.clone())
                }
                Err(f) => {
                    error!("{}; websocket keeps its current settings", f.message);
                    current.cloned()
                }
            },
            (Some(n), false) => {
                let conflicts = |a| crate::config::check::binds_conflict(a, n.listen);
                let freed = self.websocket.is_some()
                    && (current.is_some_and(|c| conflicts(c.listen))
                        || self.websocket_addr().is_some_and(conflicts));
                if freed {
                    self.stop_websocket().await;
                }
                match bind_ws_with_retry(n, if freed { FREED_TRIES } else { 1 }).await {
                    Ok(l) => {
                        self.stop_websocket().await;
                        let t = WsTask::start(&self.shared, l);
                        info!("websocket listener now on {}", t.listener.url_hint());
                        self.websocket = Some(t);
                        Some(n.clone())
                    }
                    Err(f) => self.keep_websocket(current, freed, &f.message).await,
                }
            }
            (None, _) => {
                if let Some(old) = self.websocket.take() {
                    old.task.abort();
                    info!("websocket listener stopped");
                }
                None
            }
        }
    }

    async fn stop_websocket(&mut self) {
        if let Some(old) = self.websocket.take() {
            old.task.abort();
            let _ = old.task.await;
        }
    }

    async fn keep_websocket(
        &mut self,
        current: Option<&WebSocketSettings>,
        freed: bool,
        why: &str,
    ) -> Option<WebSocketSettings> {
        let Some(c) = current.filter(|_| freed || self.websocket.is_some()) else {
            error!("{why}; websocket stays off");
            return None;
        };
        if !freed {
            error!(
                "{why}; websocket stays on {}",
                self.websocket
                    .as_ref()
                    .map_or_else(String::new, |t| t.listener.url_hint())
            );
            return Some(c.clone());
        }
        match bind_ws_with_retry(c, FREED_TRIES).await {
            Ok(l) => {
                let t = WsTask::start(&self.shared, l);
                error!("{why}; websocket stays on {}", t.listener.url_hint());
                self.websocket = Some(t);
                Some(c.clone())
            }
            Err(f) => {
                error!(
                    "{why}, and its previous address is no longer free either ({}); websocket clients cannot connect until the config is fixed or the server restarts",
                    f.message
                );
                None
            }
        }
    }
}

impl Server {
    async fn apply_metrics(&mut self, next: Option<SocketAddr>) {
        let Some(addr) = next else {
            if let Some(old) = self.metrics.take() {
                old.task.abort();
                let _ = old.task.await;
                info!("metrics endpoint stopped");
            }
            return;
        };
        let clashes = self
            .metrics
            .as_ref()
            .is_some_and(|m| crate::config::check::binds_conflict(m.addr, addr));
        if clashes && let Some(old) = self.metrics.take() {
            old.task.abort();
            let _ = old.task.await;
        }
        let mut last = None;
        for _ in 0..if clashes { FREED_TRIES } else { 1 } {
            match bind_metrics(addr) {
                Ok(l) => {
                    if let Some(old) = self.metrics.take() {
                        old.task.abort();
                        let _ = old.task.await;
                    }
                    let m = MetricsTask::start(&self.shared, l);
                    info!("metrics now on http://{}/metrics", m.addr);
                    self.metrics = Some(m);
                    return;
                }
                Err(f) => last = Some(f),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Some(f) = last {
            match &self.metrics {
                Some(m) => error!("{}; metrics stay on http://{}/metrics", f.message, m.addr),
                None => error!("{}", f.message),
            }
        }
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
        extras.push(format!("limited to {}", crate::ratelimit::describe(rate)));
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
                crate::config::model::Mode::Failover => "failover",
                crate::config::model::Mode::Balance => "balance",
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
        svc.protocol.as_str(),
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
            hint.ends_with(&crate::service::ports::inspect_hint(Protocol::Tcp, 7835)),
            "{hint}"
        );
        assert_eq!(
            hint.contains("burrow service list"),
            crate::service::any_installed(),
            "{hint}"
        );
        let udp = bind_error("service \"dns\"", Protocol::Udp, addr, &busy)
            .hint
            .unwrap();
        assert!(
            udp.ends_with(&crate::service::ports::inspect_hint(Protocol::Udp, 7835)),
            "{udp}"
        );
        if !cfg!(windows) {
            assert_ne!(hint, udp, "ss and lsof take a protocol flag");
        }
    }
}
