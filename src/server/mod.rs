mod session;
mod udp;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Notify, Semaphore, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::config::model::Protocol;
use crate::config::settings::{ServerSettings, Service};
use crate::failure::Failure;
use crate::forward::{ForwardError, server_tcp};
use crate::handshake::{AcceptError, Accepted, accept};
use crate::net::tune;
use crate::protocol::{
    ByeReason, Connect, Hello, ServerMsg, ServiceInfo, Status, TOKEN_LEN, VERSION,
};
use crate::secret::Secret;
pub use session::{ClientSession, PoolConfig, TakeError};

const TAKE_TIMEOUT: Duration = Duration::from_secs(10);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_secs(5);

pub struct Shared {
    settings: RwLock<Arc<ServerSettings>>,
    sessions: Mutex<HashMap<String, Arc<ClientSession>>>,
    active: AtomicUsize,
    drained: Notify,
    rejected: AtomicUsize,
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
    listener: TcpListener,
    services: HashMap<String, ServiceTask>,
    shutdown_tx: watch::Sender<bool>,
}

pub fn bind_error(what: &str, addr: SocketAddr, e: &std::io::Error) -> Failure {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => Failure::new(format!(
            "cannot bind {what} on {addr}: address already in use"
        ))
        .hint(format!(
            "pick another port, or find what is using it with: ss -ltnp 'sport = :{}'",
            addr.port()
        )),
        std::io::ErrorKind::PermissionDenied => {
            Failure::new(format!("cannot bind {what} on {addr}: permission denied"))
                .hint("ports below 1024 need root or CAP_NET_BIND_SERVICE; use a port above 1024")
        }
        std::io::ErrorKind::AddrNotAvailable => Failure::new(format!(
            "cannot bind {what} on {addr}: address not available on this machine"
        ))
        .hint("use 0.0.0.0 or an IP address assigned to this machine (see: ip addr)"),
        _ => Failure::new(format!("cannot bind {what} on {addr}: {e}")),
    }
}

async fn bind_service(name: &str, svc: &Service) -> Result<Bound, Failure> {
    let what = format!("service \"{name}\"");
    match svc.protocol {
        Protocol::Tcp => TcpListener::bind(svc.bind)
            .await
            .map(Bound::Tcp)
            .map_err(|e| bind_error(&what, svc.bind, &e)),
        Protocol::Udp => UdpSocket::bind(svc.bind)
            .await
            .map(Bound::Udp)
            .map_err(|e| bind_error(&what, svc.bind, &e)),
    }
}

impl Server {
    pub async fn bind(settings: ServerSettings) -> Result<Server, Failure> {
        let listener = TcpListener::bind(settings.listen)
            .await
            .map_err(|e| bind_error("the control port", settings.listen, &e))?;
        let (shutdown_tx, shutdown) = watch::channel(false);
        let shared = Arc::new(Shared {
            settings: RwLock::new(Arc::new(settings.clone())),
            sessions: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
            drained: Notify::new(),
            rejected: AtomicUsize::new(0),
            shutdown,
        });
        let mut services = HashMap::new();
        for (name, svc) in &settings.services {
            let bound = bind_service(name, svc).await?;
            services.insert(name.clone(), spawn_service(&shared, name, svc, bound));
        }
        Ok(Server {
            shared,
            listener,
            services,
            shutdown_tx,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("bound listener has an address")
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
        info!("listening for clients on {}", self.local_addr());
        for (name, t) in &self.services {
            log_service(name, &t.service, t.addr);
        }
        let permits = Arc::new(Semaphore::new(s.max_handshakes));
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.tick().await;
        loop {
            tokio::select! {
                r = self.listener.accept() => match r {
                    Ok((stream, peer)) => {
                        let Ok(permit) = permits.clone().try_acquire_owned() else {
                            self.shared.rejected.fetch_add(1, Ordering::Relaxed);
                            debug!(%peer, "too many unauthenticated connections, dropping");
                            continue;
                        };
                        let shared = self.shared.clone();
                        tokio::spawn(async move {
                            handle_connection(shared, stream, peer, permit).await;
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
            warn!(
                "changing \"listen\" from {} to {} needs a restart; keeping {}",
                current.listen, next.listen, current.listen
            );
        }
        let mut next = next;
        next.listen = current.listen;
        *self.shared.settings.write().expect("settings lock") = Arc::new(next.clone());

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
            tokio::spawn(tcp_service(shared, name_owned, l, rx)),
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

async fn tcp_service(
    shared: Arc<Shared>,
    name: String,
    listener: TcpListener,
    config: watch::Receiver<Service>,
) {
    let count = Arc::new(AtomicUsize::new(0));
    loop {
        let (visitor, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("service \"{name}\": accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let svc = config.borrow().clone();
        let shared = shared.clone();
        let name = name.clone();
        let count = count.clone();
        tokio::spawn(async move {
            visit(shared, name, svc, visitor, peer, count).await;
        });
    }
}

struct CountGuard(Arc<AtomicUsize>);

impl Drop for CountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn visit(
    shared: Arc<Shared>,
    name: String,
    svc: Service,
    visitor: TcpStream,
    peer: SocketAddr,
    count: Arc<AtomicUsize>,
) {
    let settings = shared.settings();
    let n = count.fetch_add(1, Ordering::AcqRel);
    let _guard = CountGuard(count);
    if svc.max_connections.is_some_and(|m| n >= m) {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        debug!(%peer, "service \"{name}\" is at its connection limit");
        return;
    }
    let Some(session) = shared.session(&svc.client) else {
        debug!(%peer, "service \"{name}\": client \"{}\" is not connected, dropping visitor", svc.client);
        return;
    };
    let client_limit = settings
        .clients
        .get(&svc.client)
        .and_then(|c| c.max_connections)
        .unwrap_or(settings.max_connections_per_client);
    if session.active.load(Ordering::Relaxed) >= client_limit {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        debug!(%peer, "client \"{}\" is at its connection limit", svc.client);
        return;
    }
    let _active = session.connection_guard();
    let _tracked = shared.track();
    tune(&visitor, &settings.tcp);
    let ch = match tokio::time::timeout(TAKE_TIMEOUT, session.take()).await {
        Ok(Ok(ch)) => ch,
        Ok(Err(TakeError::TooManyPending)) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            warn!(
                "service \"{name}\": too many visitors waiting for client \"{}\", dropping one",
                svc.client
            );
            return;
        }
        Ok(Err(TakeError::Closed)) => return,
        Err(_) => {
            warn!(
                "service \"{name}\": client \"{}\" did not open a data channel within {}s",
                svc.client,
                TAKE_TIMEOUT.as_secs()
            );
            return;
        }
    };
    let connect = Connect {
        protocol: Protocol::Tcp,
        service: name.clone(),
        target: svc.target.clone(),
    };
    match server_tcp(ch, visitor, &connect, settings.insecure_plaintext).await {
        Ok(()) => debug!(%peer, "service \"{name}\": connection closed"),
        Err(ForwardError::Status(st)) => log_status(&name, &svc, st),
        Err(ForwardError::Protocol) => warn!("service \"{name}\": client sent an invalid reply"),
        Err(ForwardError::Io(e)) => debug!(%peer, "service \"{name}\": connection ended: {e}"),
    }
}

pub fn log_status(name: &str, svc: &Service, st: Status) {
    let client = &svc.client;
    let target = &svc.target;
    match st {
        Status::Denied => warn!(
            "service \"{name}\": client \"{client}\" refused {target} because it is not in its allow list (add it to \"allow\" in the client config)"
        ),
        Status::Unreachable => warn!(
            "service \"{name}\": client \"{client}\" could not connect to {target} (is it running and reachable from the client?)"
        ),
        Status::Unresolved => {
            warn!("service \"{name}\": client \"{client}\" could not resolve {target}")
        }
        Status::Invalid => {
            warn!("service \"{name}\": client \"{client}\" rejected the target {target} as invalid")
        }
        Status::Ok => {}
    }
}

fn secrets_for(shared: &Shared, name: &str) -> Vec<Secret> {
    shared
        .settings()
        .clients
        .get(name)
        .map(|c| c.secrets.clone())
        .unwrap_or_default()
}

async fn handle_connection(
    shared: Arc<Shared>,
    stream: TcpStream,
    peer: SocketAddr,
    permit: tokio::sync::OwnedSemaphorePermit,
) {
    let settings = shared.settings();
    tune(&stream, &settings.tcp);
    let result = tokio::time::timeout(
        settings.handshake_timeout,
        accept(stream, |n| secrets_for(&shared, n)),
    )
    .await;
    drop(permit);
    let acc = match result {
        Ok(Ok(a)) => a,
        Ok(Err(AcceptError::Rejected)) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "handshake rejected (wrong secret or unknown client name)");
            return;
        }
        Ok(Err(e)) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "handshake failed: {e:?}");
            return;
        }
        Err(_) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "handshake timed out");
            return;
        }
    };
    match acc.hello {
        Hello::Control { version } if version == VERSION => {
            control_session(shared, acc, peer).await
        }
        Hello::Data { version, token } if version == VERSION => {
            if acc.preamble.plaintext != settings.insecure_plaintext {
                return;
            }
            match shared.session(&acc.preamble.name) {
                Some(s) if s.token == token => s.offer(acc.channel),
                _ => debug!(%peer, "data channel for an unknown session, closing"),
            }
        }
        _ => debug!(%peer, "unsupported protocol version"),
    }
}

async fn control_session(shared: Arc<Shared>, acc: Accepted, peer: SocketAddr) {
    let Accepted {
        preamble, channel, ..
    } = acc;
    let name = preamble.name.clone();
    let settings = shared.settings();
    let (stream, mut writer, mut reader) = (channel.stream, channel.writer, channel.reader);
    let (mut rd, mut wr) = stream.into_split();

    if preamble.plaintext != settings.insecure_plaintext {
        warn!(
            "client \"{name}\" from {peer} has insecure_plaintext {} but the server has it {}; disconnecting",
            on_off(preamble.plaintext),
            on_off(settings.insecure_plaintext)
        );
        let _ = writer
            .send(
                &mut wr,
                &ServerMsg::Bye {
                    reason: ByeReason::PlaintextMismatch,
                }
                .encode(),
            )
            .await;
        return;
    }

    let mut token = [0u8; TOKEN_LEN];
    getrandom::fill(&mut token).expect("operating system random source is available");
    let (tx, mut rx) = mpsc::channel::<ServerMsg>(256);
    let session = Arc::new(ClientSession::new(
        name.clone(),
        token,
        tx,
        &Shared::pool_config(&settings),
    ));
    let old = shared
        .sessions
        .lock()
        .expect("sessions lock")
        .insert(name.clone(), session.clone());
    if let Some(old) = old {
        warn!("client \"{name}\" connected again from {peer}; closing its previous connection");
        let _ = old.control.try_send(ServerMsg::Bye {
            reason: ByeReason::Replaced,
        });
        old.close();
    }
    info!("client \"{name}\" connected from {peer}");

    let interval = settings.heartbeat_interval;
    let welcome = ServerMsg::Welcome {
        token,
        heartbeat_secs: interval.as_secs().clamp(1, u16::MAX as u64) as u16,
    };
    let services = ServerMsg::Services(Shared::services_for(&settings, &name));
    let mut stop = shared.shutdown.clone();
    let writer_session = session.clone();
    let writer_task = tokio::spawn(async move {
        if writer.send(&mut wr, &welcome.encode()).await.is_err()
            || writer.send(&mut wr, &services.encode()).await.is_err()
        {
            return;
        }
        writer_session.start();
        let mut ping = tokio::time::interval(interval);
        let mut tick = tokio::time::interval(TICK);
        loop {
            let msg = tokio::select! {
                m = rx.recv() => match m {
                    Some(m) => m,
                    None => return,
                },
                _ = ping.tick() => ServerMsg::Ping,
                _ = tick.tick() => {
                    writer_session.tick();
                    continue;
                }
            };
            let bye = matches!(msg, ServerMsg::Bye { .. });
            if writer.send(&mut wr, &msg.encode()).await.is_err() || bye {
                return;
            }
        }
    });

    let timeout = interval * 3;
    let reason = loop {
        tokio::select! {
            r = tokio::time::timeout(timeout, reader.recv(&mut rd)) => match r {
                Ok(Ok(Some(_))) => continue,
                Ok(Ok(None)) => break "disconnected".to_string(),
                Ok(Err(e)) => break format!("connection error: {e}"),
                Err(_) => break format!("no heartbeat for {}s", timeout.as_secs()),
            },
            _ = stop.changed() => break "server shutting down".to_string(),
        }
    };
    let current = {
        let mut sessions = shared.sessions.lock().expect("sessions lock");
        let current = sessions
            .get(&name)
            .is_some_and(|s| Arc::ptr_eq(s, &session));
        if current {
            sessions.remove(&name);
        }
        current
    };
    session.close();
    if current {
        info!("client \"{name}\" disconnected ({reason})");
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), writer_task).await;
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}
