use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{Notify, Semaphore, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use super::listen::WsListener;
use super::session::TakeError;
use super::{
    ApplyReport, Counts, Server, Shared, TAKE_TIMEOUT, health, listen, local, standing, tcp, udp,
    via,
};
use crate::base::failure::Failure;
use crate::config::settings::WebSocketSettings;
use crate::config::settings::{ServerSettings, Service};
use burrow_config::Protocol;

pub(super) enum Bound {
    Tcp(TcpListener),
    Udp(UdpSocket),
}

pub(super) struct ServiceTask {
    pub(super) service: Service,
    pub(super) config: watch::Sender<Arc<Service>>,
    pub(super) tasks: Vec<JoinHandle<()>>,
    pub(super) addr: Option<SocketAddr>,
}

impl ServiceTask {
    pub(super) async fn stop(self) {
        for t in &self.tasks {
            t.abort();
        }
        for t in self.tasks {
            let _ = t.await;
        }
    }
}

#[derive(Default)]
pub(super) struct Wanted {
    pub(super) websocket: Option<WebSocketSettings>,
    pub(super) metrics: Option<SocketAddr>,
    #[cfg(feature = "web")]
    pub(super) web: Option<crate::config::settings::WebSettings>,
}

pub(super) struct MetricsTask {
    pub(super) addr: SocketAddr,
    pub(super) task: JoinHandle<()>,
}

impl MetricsTask {
    pub(super) fn start(shared: &Arc<Shared>, listener: TcpListener) -> MetricsTask {
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

pub(super) fn bind_metrics(addr: SocketAddr) -> Result<TcpListener, Failure> {
    crate::wire::transport::tcp::bind_tcp(addr)
        .map_err(|e| bind_error("the metrics endpoint", Protocol::Tcp, addr, &e))
}

pub(super) struct WsTask {
    pub(super) listener: Arc<WsListener>,
    pub(super) task: JoinHandle<()>,
}

impl WsTask {
    pub(super) fn start(shared: &Arc<Shared>, listener: WsListener) -> WsTask {
        let listener = Arc::new(listener);
        let task = tokio::spawn(listen::ws_accept_loop(shared.clone(), listener.clone()));
        WsTask { listener, task }
    }
}

pub(super) async fn accept_tcp(
    listener: &Option<TcpListener>,
) -> std::io::Result<(tokio::net::TcpStream, SocketAddr)> {
    match listener {
        Some(l) => l.accept().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn take_channel(
    shared: &Shared,
    session: &via::Via,
    label: &str,
    (peer, source): (SocketAddr, Option<std::net::IpAddr>),
) -> Result<crate::wire::transport::Channel, crate::wire::protocol::Status> {
    use crate::wire::protocol::Status;
    match session.take(TAKE_TIMEOUT, source).await {
        Ok(ch) => Ok(ch),
        Err(TakeError::TooManyPending) => {
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
        Err(TakeError::Closed) => {
            tracing::debug!(%peer, "{label}: session closed while waiting for a data channel");
            Err(Status::Offline)
        }
        Err(TakeError::TimedOut) => {
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

pub(super) async fn bind_service(name: &str, svc: &Service) -> Result<Vec<(u16, Bound)>, Failure> {
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

pub(super) async fn bind_ws_with_retry(
    ws: &crate::config::settings::WebSocketSettings,
    tries: usize,
) -> Result<WsListener, Failure> {
    crate::base::ports::retry(tries, || async { WsListener::bind(ws) }).await
}

pub(super) const FREED_TRIES: usize = 20;

pub(super) const BIND_RETRY: Duration = Duration::from_secs(10);

pub(super) async fn bind_with_retry(
    name: &str,
    svc: &Service,
    tries: usize,
) -> Result<Vec<(u16, Bound)>, Failure> {
    crate::base::ports::retry(tries, || bind_service(name, svc)).await
}

pub(super) fn log_service(name: &str, svc: &Service, addr: Option<SocketAddr>) {
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
                burrow_config::Mode::LeastConnections => "least connections",
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

pub(super) fn spawn_service(
    shared: &Arc<Shared>,
    name: &str,
    svc: &Service,
    bound: Vec<(u16, Bound)>,
) -> ServiceTask {
    let (config, rx) = watch::channel(Arc::new(svc.clone()));
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

impl Server {
    pub async fn bind(settings: ServerSettings) -> Result<Server, Failure> {
        let listener = match settings.listen {
            Some(addr) => Some(
                crate::wire::transport::tcp::bind_tcp(addr)
                    .map_err(|e| bind_error("the control port", Protocol::Tcp, addr, &e))?,
            ),
            None => None,
        };
        let ws_listener = match settings.websocket.as_ref().filter(|w| w.host.is_none()) {
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
            handshake_sources: burrow_syntax::share::Shares::default(),
            visitor_shares: super::share::VisitorShares::default(),
            shutdown,
            reloaded: watch::Sender::new(0),
            applied: watch::Sender::new(ApplyReport::default()),
            admin: std::sync::OnceLock::new(),
            service_counts: Counts::default(),
            visitor_counts: Counts::default(),
            client_counts: Counts::default(),
            entry_counts: Default::default(),
            limits: Mutex::new(HashMap::new()),
            probes: Mutex::new(HashMap::new()),
            #[cfg(feature = "web")]
            notifier: super::notify::Notifier::default(),
            web_wait: Mutex::new(None),
            geo: super::geo::Geo::default(),
            standing: Mutex::new(standing::Standing::default()),
            wheels: Mutex::default(),
            local: Arc::new(local::SelfDialer::new(
                &settings,
                Vec::new(),
                crate::wire::transport::resolve::system(),
            )),
            metrics: crate::metrics::Metrics::default(),
            #[cfg(feature = "web")]
            certs: certificate_store(),
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
        let web = super::web::WebTask::bind(&shared, &settings).await?;
        let health = tokio::spawn(health::run(shared.clone()));
        #[cfg(feature = "web")]
        let notify = tokio::spawn(super::notify::run(shared.clone()));
        let metrics = metrics_listener.map(|l| MetricsTask::start(&shared, l));
        let geo = super::geo::start(&shared, settings.geoip.as_ref());
        let server = Server {
            geo,
            health,
            #[cfg(feature = "web")]
            notify,
            metrics,
            shared,
            listener,
            websocket,
            web,
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
            self.web_addr(),
            self.web_http_addr(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    pub(super) fn refresh_self(&self) {
        self.shared
            .local
            .reconfigure(&self.shared.settings(), self.own_addrs());
    }
}

#[cfg(feature = "web")]
pub(crate) fn private_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    crate::base::fs::write_atomic(path, bytes, 0o600)
}

#[cfg(feature = "web")]
fn certificate_store() -> Arc<burrow_web::tls::Store> {
    burrow_web::tls::acme::storage::use_private_writer(Arc::new(private_write));
    burrow_web::tls::Store::new()
}
