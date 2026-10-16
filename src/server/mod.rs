mod control;
mod listen;
mod session;
mod tcp;
mod udp;

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
use crate::config::settings::{ServerSettings, Service};
use crate::failure::Failure;
use crate::protocol::{ByeReason, ServerMsg, ServiceInfo};
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
    handshakes: Arc<Semaphore>,
    shutdown: watch::Receiver<bool>,
}

impl Shared {
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

    fn services_for(s: &ServerSettings, client: &str) -> Vec<ServiceInfo> {
        s.services
            .iter()
            .filter(|(_, svc)| svc.client == client)
            .map(|(name, svc)| ServiceInfo {
                name: name.clone(),
                protocol: svc.protocol,
                bind_port: svc.bind.port(),
                target: svc.target.clone(),
            })
            .collect()
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
    task: JoinHandle<()>,
    addr: Option<SocketAddr>,
}

pub struct Server {
    shared: Arc<Shared>,
    listener: Option<TcpListener>,
    websocket: Option<WsTask>,
    services: HashMap<String, ServiceTask>,
    shutdown_tx: watch::Sender<bool>,
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

pub fn bind_error(what: &str, proto: Protocol, addr: SocketAddr, e: &std::io::Error) -> Failure {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => {
            let find = crate::service::ports::inspect_hint(proto, addr.port());
            let hint = if crate::service::any_installed() {
                format!(
                    "a burrow service is installed here and may be using it: burrow service status; otherwise find what uses it with: {find}"
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

async fn bind_service(name: &str, svc: &Service) -> Result<Bound, Failure> {
    let what = format!("service \"{name}\"");
    match svc.protocol {
        Protocol::Tcp => crate::transport::tcp::bind_tcp(svc.bind)
            .map(Bound::Tcp)
            .map_err(|e| bind_error(&what, Protocol::Tcp, svc.bind, &e)),
        Protocol::Udp => UdpSocket::bind(svc.bind)
            .await
            .map(Bound::Udp)
            .map_err(|e| bind_error(&what, Protocol::Udp, svc.bind, &e)),
    }
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
        let (shutdown_tx, shutdown) = watch::channel(false);
        let shared = Arc::new(Shared {
            settings: RwLock::new(Arc::new(settings.clone())),
            sessions: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
            udp_peers: AtomicUsize::new(0),
            drained: Notify::new(),
            rejected: AtomicUsize::new(0),
            handshakes: Arc::new(Semaphore::new(settings.max_handshakes)),
            shutdown,
        });
        let mut services = HashMap::new();
        for (name, svc) in &settings.services {
            let bound = bind_service(name, svc).await?;
            services.insert(name.clone(), spawn_service(&shared, name, svc, bound));
        }
        let websocket = ws_listener.map(|l| WsTask::start(&shared, l));
        Ok(Server {
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
        for (name, t) in &self.services {
            log_service(name, &t.service, t.addr);
        }
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.tick().await;
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
                        warn!("accept failed: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
                Some(next) = reload.recv() => self.apply(next).await,
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

    async fn shutdown(mut self) {
        info!("shutting down");
        let _ = self.shutdown_tx.send(true);
        if let Some(ws) = self.websocket.take() {
            ws.task.abort();
        }
        for (_, t) in self.services.drain() {
            t.task.abort();
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
        *self.shared.settings.write().expect("settings lock") = Arc::new(next.clone());

        if next.websocket != current.websocket {
            self.apply_websocket(current.websocket.as_ref(), next.websocket.as_ref())
                .await;
        }

        let removed: Vec<String> = self
            .services
            .keys()
            .filter(|k| !next.services.contains_key(*k))
            .cloned()
            .collect();
        for name in removed {
            if let Some(t) = self.services.remove(&name) {
                t.task.abort();
                info!("service \"{name}\" removed");
            }
        }
        for (name, svc) in &next.services {
            let same_socket = self
                .services
                .get(name)
                .is_some_and(|t| t.service.bind == svc.bind && t.service.protocol == svc.protocol);
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
                old.task.abort();
                let _ = old.task.await;
            }
            match bind_with_retry(name, svc).await {
                Ok(bound) => {
                    let t = spawn_service(&self.shared, name, svc, bound);
                    info!("service \"{name}\" added");
                    log_service(name, svc, t.addr);
                    self.services.insert(name.clone(), t);
                }
                Err(f) => error!("{}", f.message),
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
        for s in sessions {
            if !next.clients.contains_key(&s.name) {
                info!(
                    "client \"{}\" removed from the config, disconnecting it",
                    s.name
                );
                let _ = s.control.try_send(ServerMsg::Bye {
                    reason: ByeReason::Removed,
                });
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
                .try_send(ServerMsg::Services(Shared::services_for(&next, &s.name)));
        }
        info!(
            "config reloaded ({} clients, {} services)",
            next.clients.len(),
            next.services.len()
        );
    }
}

impl Server {
    async fn apply_websocket(
        &mut self,
        current: Option<&crate::config::settings::WebSocketSettings>,
        next: Option<&crate::config::settings::WebSocketSettings>,
    ) {
        let same_socket = self.websocket.is_some()
            && matches!((current, next), (Some(c), Some(n)) if c.listen == n.listen);
        match (next, same_socket) {
            (Some(n), true) => match listen::WsRoute::new(n) {
                Ok(route) => {
                    let t = self.websocket.as_ref().expect("checked above");
                    t.listener.set_route(route);
                    info!("websocket listener updated: {}", t.listener.url_hint());
                }
                Err(f) => error!("{}", f.message),
            },
            (Some(n), false) => {
                if let Some(old) = self.websocket.take() {
                    old.task.abort();
                    let _ = old.task.await;
                }
                match bind_ws_with_retry(n).await {
                    Ok(l) => {
                        let t = WsTask::start(&self.shared, l);
                        info!("websocket listener now on {}", t.listener.url_hint());
                        self.websocket = Some(t);
                    }
                    Err(f) => error!("{}", f.message),
                }
            }
            (None, _) => {
                if let Some(old) = self.websocket.take() {
                    old.task.abort();
                    info!("websocket listener stopped");
                }
            }
        }
    }
}

async fn bind_ws_with_retry(
    ws: &crate::config::settings::WebSocketSettings,
) -> Result<WsListener, Failure> {
    let mut last = None;
    for _ in 0..20 {
        match WsListener::bind(ws) {
            Ok(l) => return Ok(l),
            Err(f) => last = Some(f),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(last.expect("at least one attempt"))
}

async fn bind_with_retry(name: &str, svc: &Service) -> Result<Bound, Failure> {
    let mut last = None;
    for _ in 0..20 {
        match bind_service(name, svc).await {
            Ok(b) => return Ok(b),
            Err(f) => last = Some(f),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(last.expect("at least one attempt"))
}

fn log_service(name: &str, svc: &Service, addr: Option<SocketAddr>) {
    info!(
        "service \"{name}\": {} {} -> client \"{}\" {}",
        svc.protocol.as_str(),
        addr.unwrap_or(svc.bind),
        svc.client,
        svc.target
    );
}

fn spawn_service(shared: &Arc<Shared>, name: &str, svc: &Service, bound: Bound) -> ServiceTask {
    let (config, rx) = watch::channel(svc.clone());
    let shared = shared.clone();
    let name_owned = name.to_string();
    let (addr, task) = match bound {
        Bound::Tcp(l) => (
            l.local_addr().ok(),
            tokio::spawn(tcp::tcp_service(shared, name_owned, l, rx)),
        ),
        Bound::Udp(sock) => (
            sock.local_addr().ok(),
            tokio::spawn(udp::udp_service(shared, name_owned, sock, rx)),
        ),
    };
    ServiceTask {
        service: svc.clone(),
        config,
        task,
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
            hint.contains("burrow service status"),
            crate::service::any_installed(),
            "{hint}"
        );
        let udp = bind_error("service \"dns\"", Protocol::Udp, addr, &busy).hint.unwrap();
        assert!(
            udp.ends_with(&crate::service::ports::inspect_hint(Protocol::Udp, 7835)),
            "{udp}"
        );
        assert_ne!(hint, udp);
    }
}
