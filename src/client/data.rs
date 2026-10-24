use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::base::failure::io_reason;
use crate::config::check::{parse_host_port, unix};
use crate::config::settings::{ClientSettings, Limit};
use crate::forward::{client_tcp, refuse};
use crate::policy::allowlist::{AllowList, Gate, Refusal};
use crate::policy::throttle::Throttle;
use crate::wire::protocol::{
    Connect, KEEPALIVE_RECORD, MAX_DATAGRAM, STALE_RECORD, ServiceInfo, Status, decode_datagram,
    encode_datagram,
};
use crate::wire::transport::Channel;
use crate::wire::transport::resolve::Resolver;
use crate::wire::transport::tcp::{SocketTuning, connect_any, tune};
use burrow_config::Protocol;

#[derive(Clone, PartialEq)]
pub(crate) struct Dialing {
    pub(crate) gate: Gate,
    pub(crate) connect_timeout: Duration,
    pub(crate) tcp: SocketTuning,
    pub(crate) udp_session_timeout: Duration,
    pub(crate) plaintext: bool,
    pub(crate) list: &'static str,
}

impl Dialing {
    pub(crate) fn of_client(s: &ClientSettings) -> Dialing {
        Dialing {
            gate: Gate::client(s.allow.clone()),
            connect_timeout: s.connect_timeout,
            tcp: s.tcp.clone(),
            udp_session_timeout: s.udp_session_timeout,
            plaintext: s.insecure_plaintext,
            list: "\"allow\"",
        }
    }
}

pub(crate) trait Dials: Send + Sync + 'static {
    fn dialing(&self) -> Arc<Dialing>;
}

pub(crate) struct Rules<W>(
    pub(crate) Arc<tokio::sync::watch::Sender<W>>,
    pub(crate) Open,
);

impl<W> Clone for Rules<W> {
    fn clone(&self) -> Self {
        Rules(self.0.clone(), self.1.clone())
    }
}

impl<W: Dials> Rules<W> {
    fn now(&self) -> Arc<Dialing> {
        self.0.borrow().dialing()
    }
}

const IDLE_CHANNEL_LIMIT: Duration = Duration::from_secs(90);

struct Peer {
    sock: Arc<UdpSocket>,
    to: SocketAddr,
    seen: Arc<AtomicU64>,
    task: tokio::task::AbortHandle,
    _slot: Slot,
}

#[derive(Clone)]
pub(crate) struct Open {
    pub(crate) connections: Arc<Slots>,
    pub(crate) peers: Arc<Slots>,
    pub(crate) local: bool,
}

impl Open {
    pub(crate) fn new((connections, peers): (usize, usize), local: bool) -> Open {
        Open {
            connections: Arc::new(Slots::new(connections)),
            peers: Arc::new(Slots::new(peers)),
            local,
        }
    }
}

static OPEN_FILES: AtomicU64 = AtomicU64::new(u64::MAX);

pub(crate) struct Slots {
    open: AtomicUsize,
    max: AtomicUsize,
}

impl Slots {
    pub(crate) fn new(max: usize) -> Slots {
        Slots {
            open: AtomicUsize::new(0),
            max: AtomicUsize::new(max),
        }
    }

    fn take(self: &Arc<Self>) -> Option<Slot> {
        self.open
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.most()).then_some(n + 1)
            })
            .ok()
            .map(|_| Slot(self.clone()))
    }

    fn most(&self) -> usize {
        self.max.load(Ordering::Acquire)
    }

    pub(crate) fn limit(&self, max: usize) {
        self.max.store(max, Ordering::Release);
    }
}

pub(crate) const MAX_WAITING: usize = 4096;
pub(crate) static WAITING_CAP: AtomicUsize = AtomicUsize::new(MAX_WAITING);

fn caps(open_files: u64, connections: usize, peers: usize) -> (usize, usize, usize) {
    let budget = usize::try_from(open_files.saturating_sub(512)).unwrap_or(usize::MAX);
    let fit = |share: usize, cap: usize| (budget / 8).saturating_mul(share).max(64).min(cap);
    (fit(3, connections), fit(1, peers), fit(1, MAX_WAITING))
}

#[cfg(unix)]
pub(crate) fn fit_to_open_files(limit: u64) {
    OPEN_FILES.store(limit, Ordering::Relaxed);
    let (_, _, waiting) = caps(limit, 0, 0);
    WAITING_CAP.store(waiting, Ordering::Relaxed);
}

fn wide(n: u32) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

pub(crate) fn fitted(connections: u32, peers: u32) -> (usize, usize) {
    let (connections, peers, _) = caps(
        OPEN_FILES.load(Ordering::Relaxed),
        wide(connections),
        wide(peers),
    );
    (connections, peers)
}

pub(crate) fn client_connection_cap(configured: Limit) -> usize {
    warned("max_connections", configured, fitted(configured.value, 1).0)
}

pub(crate) fn client_peer_cap(configured: Limit) -> usize {
    warned("max_udp_peers", configured, fitted(1, configured.value).1)
}

fn warned(key: &str, configured: Limit, cap: usize) -> usize {
    if let Some(lowered) = lowered(key, configured, cap, OPEN_FILES.load(Ordering::Relaxed)) {
        warn!("{lowered}");
    }
    cap
}

fn lowered(key: &str, configured: Limit, cap: usize, open_files: u64) -> Option<String> {
    let value = configured.value;
    (configured.written && cap < wide(value)).then(|| {
        format!(
            "{key} {value} is lowered to {cap} by the open file limit ({open_files}); {}",
            way_out(key, cap, open_files)
        )
    })
}

fn way_out(key: &str, cap: usize, open_files: u64) -> String {
    let fewer = format!("set {key} to {cap} or less to silence this, or run several clients");
    if cfg!(target_os = "macos") {
        if open_files < 10240 {
            "install burrow as a service, which sets 10240, the most macOS allows".into()
        } else {
            format!("10240 is the most macOS allows; {fewer}")
        }
    } else if cfg!(target_os = "linux") && open_files < 1048576 {
        "raise the hard limit as root (limits.conf, or LimitNOFILE in a systemd unit), or install burrow as a service, which sets 1048576".into()
    } else {
        format!("raise the hard limit as root, or {fewer}")
    }
}

struct Slot(Arc<Slots>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::AcqRel);
    }
}

struct RelayCleanup {
    peers: Arc<Mutex<HashMap<u32, Peer>>>,
    writer: tokio::task::AbortHandle,
}

impl Drop for RelayCleanup {
    fn drop(&mut self) {
        if let Ok(mut map) = self.peers.lock() {
            for (_, p) in map.drain() {
                p.task.abort();
            }
        }
        self.writer.abort();
    }
}

async fn connected_socket(target: SocketAddr) -> std::io::Result<Arc<UdpSocket>> {
    let bind: SocketAddr = if target.is_ipv4() {
        (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
    };
    let sock = crate::wire::transport::udp::bind_udp(bind).await?;
    sock.connect(target).await?;
    Ok(Arc::new(sock))
}

thread_local! {
    static RECEIVE: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0u8; 65536]);
}

fn receive(sock: &UdpSocket) -> Option<Vec<u8>> {
    RECEIVE.with(|buf| {
        let mut buf = buf.borrow_mut();
        match sock.try_recv(&mut buf) {
            Ok(n) => Some(buf[..n].to_vec()),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::ConnectionRefused {
                    static REFUSED: Throttle = Throttle::new();
                    if let Some(more) = REFUSED.allow() {
                        warn!(
                            "UDP target {} refused a datagram; is anything listening there?{more}",
                            sock.peer_addr()
                                .map_or_else(|_| "?".to_string(), |a| a.to_string())
                        );
                    }
                }
                None
            }
        }
    })
}

fn preferred(addrs: &[SocketAddr]) -> SocketAddr {
    addrs
        .iter()
        .copied()
        .find(SocketAddr::is_ipv4)
        .unwrap_or(addrs[0])
}

const RESOLVE_AGAIN: Duration = Duration::from_secs(30);

async fn peer_replies<W: Dials>(
    id: u32,
    sock: Arc<UdpSocket>,
    seen: Arc<AtomicU64>,
    epoch: Instant,
    live: Rules<W>,
    tx: mpsc::Sender<(u32, Vec<u8>)>,
    peers: Arc<Mutex<HashMap<u32, Peer>>>,
) {
    loop {
        let timeout = live.now().udp_session_timeout;
        let check = timeout.min(Duration::from_secs(5));
        match tokio::time::timeout(check, sock.readable()).await {
            Ok(Ok(())) => {
                let Some(datagram) = receive(&sock) else {
                    continue;
                };
                if datagram.len() > MAX_DATAGRAM {
                    static LARGE: Throttle = Throttle::new();
                    if let Some(more) = LARGE.allow() {
                        warn!(
                            "UDP reply of {} bytes from {} dropped: the tunnel carries at most {MAX_DATAGRAM} bytes per datagram{more}",
                            datagram.len(),
                            sock.peer_addr()
                                .map_or_else(|_| "the target".to_string(), |a| a.to_string())
                        );
                    }
                    continue;
                }
                seen.store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
                if tx.send((id, datagram)).await.is_err() {
                    break;
                }
            }
            Ok(Err(_)) => continue,
            Err(_) => {
                let mut map = peers.lock().expect("peers lock");
                let idle = epoch
                    .elapsed()
                    .saturating_sub(Duration::from_millis(seen.load(Ordering::Relaxed)));
                if idle >= timeout {
                    if map.get(&id).is_some_and(|p| Arc::ptr_eq(&p.sock, &sock)) {
                        map.remove(&id);
                    }
                    return;
                }
            }
        }
    }
    let mut map = peers.lock().expect("peers lock");
    if map.get(&id).is_some_and(|p| Arc::ptr_eq(&p.sock, &sock)) {
        map.remove(&id);
    }
}

pub fn precheck(settings: &ClientSettings, svc: &ServiceInfo) {
    if let Some(path) = unix::path_of(&svc.target) {
        if !settings.allow.permits_socket(path) {
            warn!(
                "service \"{}\" targets {}, which the allow list does not permit; add \"{}\" to \"allow\" to expose it",
                svc.name, svc.target, svc.target
            );
        }
        return;
    }
    let Ok((host, port)) = parse_host_port(&svc.target) else {
        return;
    };
    let Ok(ip) = host.parse::<std::net::IpAddr>() else {
        return;
    };
    let last = port.saturating_add(svc.ports.max(1) - 1);
    let refused = (port..=last)
        .map(|p| SocketAddr::new(ip, p))
        .find(|a| !settings.allow.permits(*a));
    if let Some(addr) = refused {
        let hint = if last > port {
            AllowList::range_suggestion(ip, port, last)
        } else {
            AllowList::suggestion(addr)
        };
        warn!(
            "service \"{}\" targets {addr}, which the allow list does not permit; add \"{hint}\" to \"allow\" to expose it",
            svc.name,
        );
    }
}

pub(crate) async fn serve<W: Dials>(
    live: Rules<W>,
    resolver: Arc<dyn Resolver>,
    mut ch: Channel,
    mut ended: tokio::sync::watch::Receiver<bool>,
    held: super::Waiting,
) -> bool {
    let first = loop {
        let next = tokio::time::timeout(IDLE_CHANNEL_LIMIT, ch.reader.recv(&mut ch.stream));
        let rec = tokio::select! {
            r = next => match r {
                Ok(r) => r,
                Err(_) => return false,
            },
            _ = ended.wait_for(|e| *e) => return false,
        };
        match rec {
            Ok(Some([KEEPALIVE_RECORD])) => continue,
            Ok(Some([STALE_RECORD])) => return true,
            Ok(Some(b)) => break Connect::decode(b),
            _ => return false,
        }
    };
    drop(held);
    connected(live, resolver, ch, first).await;
    false
}

pub(crate) async fn serve_local<W: Dials>(
    live: Rules<W>,
    resolver: Arc<dyn Resolver>,
    mut ch: Channel,
) {
    let first = match ch.reader.recv(&mut ch.stream).await {
        Ok(Some(b)) => Connect::decode(b),
        _ => return,
    };
    connected(live, resolver, ch, first).await;
}

async fn connected<W: Dials>(
    live: Rules<W>,
    resolver: Arc<dyn Resolver>,
    ch: Channel,
    first: Result<Connect, crate::wire::protocol::DecodeError>,
) {
    let connect = match first {
        Ok(c) => c,
        Err(_) => {
            refuse(ch, Status::Invalid).await;
            return;
        }
    };
    let Some(_slot) = live.1.connections.take() else {
        static FULL: Throttle = Throttle::per_minute();
        if let Some(refused) = FULL.counted("refused") {
            let most = live.1.connections.most();
            if live.1.local {
                warn!(
                    "{most} connections from this server to its own targets are open; new ones are answered busy until some close{refused}"
                );
            } else {
                warn!(
                    "max_connections {most} reached; new connections are answered busy until some close{refused}; raise max_connections in client.yaml"
                );
            }
        }
        refuse(ch, Status::Busy).await;
        return;
    };
    let settings = live.now();
    if let Some(path) = unix::path_of(&connect.target) {
        socket(&live, &settings, ch, &connect, path).await;
        return;
    }
    let addrs = match permitted(&settings, &resolver, &connect).await {
        Ok(a) => a,
        Err(status) => {
            refuse(ch, status).await;
            return;
        }
    };
    match connect.protocol {
        Protocol::Tcp => {
            let target = match connect_any(&addrs, settings.connect_timeout).await {
                Ok(t) => t,
                Err(e) => {
                    static UNREACHABLE: Throttle = Throttle::new();
                    if let Some(more) = UNREACHABLE.allow() {
                        warn!(
                            "{}: cannot connect to {} ({}); {}{more}",
                            label(&connect),
                            connect.target,
                            io_reason(&e),
                            unreachable_hint(&e)
                        );
                    }
                    refuse(ch, Status::Unreachable).await;
                    return;
                }
            };
            tune(&target, &settings.tcp);
            let Ok(peer) = target.peer_addr() else {
                return;
            };
            tokio::select! {
                r = client_tcp(ch, target, settings.plaintext) => {
                    if let Err(e) = r {
                        debug!("{}: connection ended: {e:?}", label(&connect));
                    }
                }
                _ = revoked(&live, |s| s.gate.permits(Protocol::Tcp, peer)) => {
                    no_longer_allowed(&connect);
                }
            }
        }
        Protocol::Udp => {
            let resolved = Arc::new(Mutex::new((preferred(&addrs), Instant::now())));
            let (watched, current) = (live.clone(), resolved.clone());
            let label = connect.clone();
            tokio::select! {
                _ = udp(live, resolver, ch, connect, resolved) => {}
                _ = revoked(&watched, |s| {
                    s.gate.permits(Protocol::Udp, current.lock().expect("resolved lock").0)
                }) => {
                    no_longer_allowed(&label);
                }
            }
        }
    }
}

async fn revoked<W: Dials>(live: &Rules<W>, allowed: impl Fn(&Dialing) -> bool) {
    let mut rx = live.0.subscribe();
    loop {
        let now = rx.borrow_and_update().dialing();
        if !allowed(&now) {
            return;
        }
        if rx.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
}

fn no_longer_allowed(connect: &Connect) {
    info!(
        "{}: {} is no longer in the allow list, so its open connection was closed",
        label(connect),
        connect.target
    );
}

async fn socket<W: Dials>(
    live: &Rules<W>,
    settings: &Dialing,
    ch: Channel,
    connect: &Connect,
    path: &str,
) {
    if connect.protocol != Protocol::Tcp || unix::check_path(path).is_err() {
        refuse(ch, Status::Invalid).await;
        return;
    }
    if let Some(only) = connect.only.as_deref()
        && !AllowList::from_rules(only).permits_socket(path)
    {
        outside_reach(connect);
        refuse(ch, Status::Denied).await;
        return;
    }
    if !settings.gate.permits_socket(path) {
        static DENIED: Throttle = Throttle::new();
        if let Some(more) = DENIED.allow() {
            warn!(
                "denied connection to {} for {}: it is not in the allow list; add \"{}\" to {} to permit it{more}",
                connect.target,
                whom(connect),
                connect.target,
                settings.list
            );
        }
        refuse(ch, Status::Denied).await;
        return;
    }
    #[cfg(unix)]
    if tokio::fs::symlink_metadata(path)
        .await
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        static LINKED: Throttle = Throttle::new();
        if let Some(more) = LINKED.allow() {
            warn!(
                "denied connection to {} for {}: it is a symbolic link, which could lead anywhere; allow the socket it points to instead{more}",
                connect.target,
                whom(connect)
            );
        }
        refuse(ch, Status::Denied).await;
        return;
    }
    #[cfg(unix)]
    if let Some(link) = foreign_link_on(std::path::Path::new(path)) {
        static FOREIGN: Throttle = Throttle::new();
        if let Some(more) = FOREIGN.allow() {
            warn!(
                "denied connection to {} for {}: the folder {} on its path is a symbolic link owned by another user{more}",
                connect.target,
                whom(connect),
                link.display()
            );
        }
        refuse(ch, Status::Denied).await;
        return;
    }
    #[cfg(unix)]
    match tokio::net::UnixStream::connect(path).await {
        Ok(target) => {
            tokio::select! {
                r = client_tcp(ch, target, settings.plaintext) => {
                    if let Err(e) = r {
                        debug!("{}: connection ended: {e:?}", label(connect));
                    }
                }
                _ = revoked(live, |s| s.gate.permits_socket(path)) => {
                    no_longer_allowed(connect);
                }
            }
        }
        Err(e) => {
            static UNREACHABLE: Throttle = Throttle::new();
            if let Some(more) = UNREACHABLE.allow() {
                warn!(
                    "{}: cannot connect to {} ({}); is it running?{more}",
                    label(connect),
                    connect.target,
                    io_reason(&e)
                );
            }
            refuse(ch, Status::Unreachable).await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = live;
        static UNSUPPORTED: Throttle = Throttle::new();
        if let Some(more) = UNSUPPORTED.allow() {
            warn!(
                "{}: {} is a Unix socket, which this system does not support{more}",
                label(connect),
                connect.target
            );
        }
        refuse(ch, Status::Invalid).await;
    }
}

async fn lookup(
    settings: &Dialing,
    resolver: &Arc<dyn Resolver>,
    connect: &Connect,
) -> std::io::Result<Vec<SocketAddr>> {
    let timed_out = || std::io::Error::new(std::io::ErrorKind::TimedOut, "the lookup timed out");
    if connect.target.parse::<SocketAddr>().is_ok() {
        return tokio::time::timeout(settings.connect_timeout, resolver.resolve(&connect.target))
            .await
            .unwrap_or_else(|_| Err(timed_out()));
    }
    let (slots, max, whose) = match connect.service.is_empty() {
        true => (&VISITOR_LOOKUPS, MAX_LOOKUPS, "visitors"),
        false => (&SERVICE_LOOKUPS, MAX_SERVICE_LOOKUPS, "services"),
    };
    let started = tokio::time::Instant::now();
    let Ok(Ok(held)) = tokio::time::timeout(settings.connect_timeout, slots.acquire()).await else {
        return Err(std::io::Error::other(format!(
            "{max} host name lookups for {whose} are still running"
        )));
    };
    let left = settings.connect_timeout.saturating_sub(started.elapsed());
    let (resolver, target) = (resolver.clone(), connect.target.clone());
    let running = tokio::spawn(async move {
        let _held = held;
        resolver.resolve(&target).await
    });
    match tokio::time::timeout(left, running).await {
        Ok(Ok(resolved)) => resolved,
        Ok(Err(_)) => Err(std::io::Error::other("the lookup stopped")),
        Err(_) => Err(timed_out()),
    }
}

#[cfg(unix)]
fn foreign_link_on(path: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let me = rustix::process::geteuid().as_raw();
    path.ancestors().skip(1).find_map(|folder| {
        let m = std::fs::symlink_metadata(folder).ok()?;
        (m.file_type().is_symlink() && m.uid() != 0 && m.uid() != me).then(|| folder.to_path_buf())
    })
}

const MAX_LOOKUPS: usize = 128;
const MAX_SERVICE_LOOKUPS: usize = 192;

static VISITOR_LOOKUPS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_LOOKUPS);
static SERVICE_LOOKUPS: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(MAX_SERVICE_LOOKUPS);

fn whom(connect: &Connect) -> String {
    match connect.service.as_str() {
        "" => "a visitor".to_string(),
        s => format!("service \"{s}\""),
    }
}

async fn permitted(
    settings: &Dialing,
    resolver: &Arc<dyn Resolver>,
    connect: &Connect,
) -> Result<Vec<SocketAddr>, Status> {
    if parse_host_port(&connect.target).is_err() {
        return Err(Status::Invalid);
    }
    let resolved = match lookup(settings, resolver, connect).await {
        Ok(a) => a,
        Err(e) => {
            static UNRESOLVED: Throttle = Throttle::new();
            if let Some(more) = UNRESOLVED.allow() {
                warn!(
                    "{}: cannot resolve {} ({}){more}",
                    label(connect),
                    connect.target,
                    io_reason(&e)
                );
            }
            return Err(if connect.service.is_empty() {
                Status::Denied
            } else {
                Status::Unresolved
            });
        }
    };
    let reach = connect.only.as_deref().map(AllowList::from_rules);
    let reachable: Vec<SocketAddr> = resolved
        .iter()
        .map(|a| SocketAddr::new(a.ip().to_canonical(), a.port()))
        .filter(|a| reach.as_ref().is_none_or(|r| r.permits(*a)))
        .collect();
    if reachable.is_empty() {
        outside_reach(connect);
        return Err(Status::Denied);
    }
    let resolved = reachable;
    let allowed: Vec<SocketAddr> = resolved
        .iter()
        .copied()
        .filter(|a| settings.gate.permits(connect.protocol, *a))
        .collect();
    if allowed.is_empty() {
        static DENIED: Throttle = Throttle::new();
        if let Some(more) = DENIED.allow() {
            let first = resolved[0];
            let whom = whom(connect);
            let why = match settings.gate.refusal(connect.protocol, first) {
                Some(Refusal::Burrow(own)) => {
                    format!("{first} is burrow's own listener {own}, which would loop")
                }
                Some(Refusal::LinkLocal) => format!(
                    "{first} is a link-local or metadata address, which {} must name on its own, for example \"{}\"",
                    settings.list,
                    AllowList::suggestion(first)
                ),
                _ => format!(
                    "{first} is not in the allow list; add \"{}\" to {} to permit it",
                    AllowList::suggestion(first),
                    settings.list
                ),
            };
            warn!(
                "denied connection to {} for {whom}: {why}{more}",
                connect.target
            );
        }
        return Err(Status::Denied);
    }
    Ok(allowed)
}

fn outside_reach(connect: &Connect) {
    static OUTSIDE: Throttle = Throttle::new();
    if let Some(more) = OUTSIDE.allow() {
        warn!(
            "denied connection to {} for a visitor: the server limits what it may reach to {}{more}",
            connect.target,
            connect.only.as_deref().unwrap_or_default().join(", ")
        );
    }
}

fn label(connect: &Connect) -> String {
    match connect.service.as_str() {
        "" => "visitor connection".to_string(),
        s => format!("service \"{s}\""),
    }
}

async fn udp<W: Dials>(
    live: Rules<W>,
    resolver: Arc<dyn Resolver>,
    ch: Channel,
    connect: Connect,
    resolved: Arc<Mutex<(SocketAddr, Instant)>>,
) {
    let Channel {
        stream,
        mut writer,
        mut reader,
    } = ch;
    let (mut rd, mut wr) = stream.into_split();
    let resolving = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if writer.send(&mut wr, &[Status::Ok as u8]).await.is_err() {
        return;
    }
    let (tx, mut rx) = mpsc::channel::<(u32, Vec<u8>)>(1024);
    let writer_task = tokio::spawn(async move {
        let mut rec = Vec::with_capacity(65536);
        while let Some((id, data)) = rx.recv().await {
            encode_datagram(id, &data, &mut rec);
            if writer.send(&mut wr, &rec).await.is_err() {
                return;
            }
        }
    });
    let epoch = Instant::now();
    let peers: Arc<Mutex<HashMap<u32, Peer>>> = Arc::new(Mutex::new(HashMap::new()));
    let _cleanup = RelayCleanup {
        peers: peers.clone(),
        writer: writer_task.abort_handle(),
    };
    loop {
        let rec = match reader.recv(&mut rd).await {
            Ok(Some(r)) => r,
            _ => break,
        };
        let Ok((id, payload)) = decode_datagram(rec) else {
            break;
        };
        let (target, at) = *resolved.lock().expect("resolved lock");
        if at.elapsed() >= RESOLVE_AGAIN && !resolving.swap(true, Ordering::AcqRel) {
            let (live, resolver, connect) = (live.clone(), resolver.clone(), connect.clone());
            let (resolved, resolving) = (resolved.clone(), resolving.clone());
            tokio::spawn(async move {
                let found = permitted(&live.now(), &resolver, &connect).await;
                let mut current = resolved.lock().expect("resolved lock");
                *current = match found {
                    Ok(addrs)
                        if addrs.contains(&current.0)
                            && current.0.is_ipv4() == preferred(&addrs).is_ipv4() =>
                    {
                        (current.0, Instant::now())
                    }
                    Ok(addrs) => (preferred(&addrs), Instant::now()),
                    Err(_) => (current.0, Instant::now()),
                };
                resolving.store(false, Ordering::Release);
            });
        }
        let existing = {
            let mut map = peers.lock().expect("peers lock");
            if map.get(&id).is_some_and(|p| p.to != target)
                && let Some(moved) = map.remove(&id)
            {
                moved.task.abort();
            }
            map.get(&id).map(|p| {
                p.seen
                    .store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
                (p.sock.clone(), p.seen.clone())
            })
        };
        let (sock, seen) = match existing {
            Some(p) => p,
            None => {
                let Some(slot) = live.1.peers.take() else {
                    static ALL: Throttle = Throttle::per_minute();
                    if let Some(dropped) = ALL.counted("dropped") {
                        let most = live.1.peers.most();
                        if live.1.local {
                            warn!(
                                "{most} UDP peers from this server to its own targets are open; datagrams from new ones are dropped until some expire{dropped}"
                            );
                        } else {
                            warn!(
                                "max_udp_peers {most} reached; datagrams from new UDP peers are dropped until some expire{dropped}; raise max_udp_peers in client.yaml"
                            );
                        }
                    }
                    continue;
                };
                let sock = match connected_socket(target).await {
                    Ok(sock) => sock,
                    Err(e) => {
                        static FAILED: Throttle = Throttle::new();
                        if let Some(more) = FAILED.allow() {
                            warn!(
                                "UDP to {target}: cannot open a socket for a new peer ({}), so its datagram was dropped{more}",
                                io_reason(&e)
                            );
                        }
                        continue;
                    }
                };
                let seen = Arc::new(AtomicU64::new(0));
                let task = tokio::spawn(peer_replies(
                    id,
                    sock.clone(),
                    seen.clone(),
                    epoch,
                    live.clone(),
                    tx.clone(),
                    peers.clone(),
                ));
                peers.lock().expect("peers lock").insert(
                    id,
                    Peer {
                        sock: sock.clone(),
                        to: target,
                        seen: seen.clone(),
                        task: task.abort_handle(),
                        _slot: slot,
                    },
                );
                (sock, seen)
            }
        };
        seen.store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        if let Err(e) = sock.send(payload).await
            && matches!(
                e.kind(),
                std::io::ErrorKind::NetworkUnreachable
                    | std::io::ErrorKind::HostUnreachable
                    | std::io::ErrorKind::AddrNotAvailable
                    | std::io::ErrorKind::NotConnected
            )
        {
            let mut map = peers.lock().expect("peers lock");
            if map.get(&id).is_some_and(|p| Arc::ptr_eq(&p.sock, &sock))
                && let Some(p) = map.remove(&id)
            {
                p.task.abort();
            }
            static SEND: Throttle = Throttle::new();
            if let Some(more) = SEND.allow() {
                warn!(
                    "UDP to {}: cannot send ({}), so the next datagram opens a new socket{more}",
                    sock.peer_addr()
                        .map_or_else(|_| "the target".to_string(), |a| a.to_string()),
                    io_reason(&e)
                );
            }
        }
    }
    debug!("service \"{}\": UDP relay closed", connect.service);
}

fn unreachable_hint(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::AddrInUse => {
            "this machine has no local port left for it, usually because many recent connections to it are still closing (TIME_WAIT)"
        }
        _ => "is it running?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_and_peer_caps_fit_the_open_file_limit() {
        let (connections, peers, waiting) = caps(10240, 16384, 8192);
        assert!(
            waiting + connections * 2 + peers <= 10240 - 512,
            "{waiting} {connections} {peers}"
        );
        let most = (16384, 8192, MAX_WAITING);
        assert_eq!(caps(1 << 20, 16384, 8192), most);
        assert_eq!(caps(256, 16384, 8192), (64, 64, 64));
        assert_eq!(caps(u64::MAX, 16384, 8192), most);
    }

    #[test]
    fn the_configured_connection_cap_is_lowered_by_the_open_file_limit_and_never_raised() {
        assert_eq!(caps(10240, 16384, 8192).0, 3648);
        assert_eq!(caps(10240, 100, 8192).0, 100);
        assert_eq!(caps(10240, 5000, 8192).0, 3648);
        assert_eq!(caps(1 << 20, 50000, 8192).0, 50000);
        assert_eq!(caps(256, 2, 8192).0, 2);
        assert_eq!(caps(256, 100, 8192).0, 64);
        assert_eq!(caps(u64::MAX, 1, 8192).0, 1);
    }

    #[test]
    fn the_configured_udp_peer_cap_is_lowered_by_the_open_file_limit_and_never_raised() {
        assert_eq!(caps(10240, 16384, 8192).1, 1216);
        assert_eq!(caps(10240, 16384, 100).1, 100);
        assert_eq!(caps(1 << 20, 16384, 50000).1, 50000);
        assert_eq!(caps(256, 16384, 3).1, 3);
        assert_eq!(caps(256, 16384, 100).1, 64);
        assert_eq!(caps(u64::MAX, 16384, 1).1, 1);
    }

    #[test]
    fn a_lowered_cap_names_both_values_the_limit_and_the_way_out() {
        let written = |n| Limit::written(n);
        let start = "max_connections 16384 is lowered to 3648 by the open file limit (10240); ";
        let told = lowered("max_connections", written(16384), 3648, 10240).unwrap();
        assert!(told.starts_with(start), "{told}");
        let way = &told[start.len()..];
        if cfg!(target_os = "macos") {
            assert_eq!(
                way,
                "10240 is the most macOS allows; set max_connections to 3648 or less to silence this, or run several clients"
            );
        } else if cfg!(target_os = "linux") {
            assert_eq!(
                way,
                "raise the hard limit as root (limits.conf, or LimitNOFILE in a systemd unit), or install burrow as a service, which sets 1048576"
            );
        } else {
            assert_eq!(
                way,
                "raise the hard limit as root, or set max_connections to 3648 or less to silence this, or run several clients"
            );
        }
        assert_eq!(lowered("max_connections", written(100), 100, 10240), None);
        assert!(
            lowered("max_udp_peers", written(8192), 1216, 10240)
                .unwrap()
                .starts_with(
                    "max_udp_peers 8192 is lowered to 1216 by the open file limit (10240); "
                )
        );
    }

    #[test]
    fn a_default_limit_is_lowered_without_a_warning() {
        assert_eq!(
            lowered("max_connections", Limit::default_of(16384), 3648, 10240),
            None
        );
        assert_eq!(
            lowered("max_udp_peers", Limit::default_of(8192), 64, 1024),
            None
        );
    }

    #[test]
    fn the_service_is_suggested_only_where_it_would_raise_the_limit() {
        let way = |files| way_out("max_connections", 64, files);
        if cfg!(target_os = "macos") {
            assert!(way(256).contains("install burrow as a service, which sets 10240"));
            assert!(!way(10240).contains("service"));
        } else if cfg!(target_os = "linux") {
            assert!(way(1024).contains("install burrow as a service, which sets 1048576"));
            assert!(!way(1048576).contains("service"));
        } else {
            assert!(!way(1024).contains("service"));
        }
    }

    #[test]
    fn slots_never_pass_the_cap_while_it_changes_and_count_back_to_zero() {
        let slots = Arc::new(Slots::new(16384));
        for cap in [4, 1, 7, 2] {
            slots.limit(cap);
            for _ in 0..50 {
                let start = Arc::new(std::sync::Barrier::new(8));
                let workers: Vec<_> = (0..8)
                    .map(|_| {
                        let (slots, start) = (slots.clone(), start.clone());
                        std::thread::spawn(move || {
                            start.wait();
                            (0..cap + 2)
                                .filter_map(|_| slots.take())
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                let held: Vec<Slot> = workers
                    .into_iter()
                    .flat_map(|w| w.join().unwrap())
                    .collect();
                assert_eq!(held.len(), cap, "racing takes passed a cap of {cap}");
                assert_eq!(slots.open.load(Ordering::Acquire), cap);
            }
            assert_eq!(slots.open.load(Ordering::Acquire), 0);
        }
        slots.limit(2);
        let two = (slots.take(), slots.take());
        assert!(two.0.is_some() && two.1.is_some());
        assert!(slots.take().is_none());
        slots.limit(1);
        assert!(slots.take().is_none());
        drop(two);
        assert_eq!(slots.open.load(Ordering::Acquire), 0);
        assert!(slots.take().is_some());
    }

    #[test]
    fn running_out_of_local_ports_is_named_as_such() {
        let ports = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable);
        assert!(unreachable_hint(&ports).contains("TIME_WAIT"));
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert_eq!(unreachable_hint(&refused), "is it running?");
    }

    #[test]
    fn udp_peers_across_all_services_share_one_limit() {
        let slots = Arc::new(Slots::new(2));
        let first = slots.take().unwrap();
        let _second = slots.take().unwrap();
        assert!(slots.take().is_none());
        drop(first);
        assert!(slots.take().is_some());
    }
    use crate::wire::transport::resolve::Resolved;

    struct Hanging;

    impl Resolver for Hanging {
        fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
            Box::pin(async move {
                if host_port.starts_with("hang.test:") {
                    std::future::pending::<()>().await;
                }
                crate::wire::transport::resolve::resolve(host_port).await
            })
        }
    }

    fn visit(target: &str) -> Connect {
        Connect {
            protocol: Protocol::Tcp,
            service: String::new(),
            target: target.into(),
            only: None,
        }
    }

    #[tokio::test]
    async fn a_mapped_ipv6_target_is_dialed_as_ipv4() {
        let mut settings = crate::client::reload::tests::settings();
        settings.allow = AllowList::Rules(vec!["127.0.0.0/8".parse().unwrap()]);
        let settings = Dialing::of_client(&settings);
        let resolver: Arc<dyn Resolver> = Arc::new(Hanging);
        let addrs = permitted(&settings, &resolver, &visit("[::ffff:127.0.0.1]:22"))
            .await
            .unwrap();
        assert_eq!(addrs, vec!["127.0.0.1:22".parse::<SocketAddr>().unwrap()]);
    }

    #[cfg(unix)]
    #[test]
    fn only_folder_links_of_other_users_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let linked = dir.path().join("linked");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        assert_eq!(foreign_link_on(&linked.join("app.sock")), None);
        assert_eq!(
            foreign_link_on(std::path::Path::new("/var/run/x.sock")),
            None
        );
        if let Some(foreign) = std::env::var_os("BURROW_TEST_FOREIGN_LINK") {
            let foreign = std::path::PathBuf::from(foreign);
            assert_eq!(foreign_link_on(&foreign.join("docker.sock")), Some(foreign));
        }
    }

    #[tokio::test]
    async fn lookups_that_never_return_cannot_use_up_more_than_their_share() {
        let mut settings = crate::client::reload::tests::settings();
        settings.connect_timeout = Duration::from_millis(50);
        let settings = Dialing::of_client(&settings);
        let resolver: Arc<dyn Resolver> = Arc::new(Hanging);
        let mut stuck = tokio::task::JoinSet::new();
        for _ in 0..MAX_LOOKUPS {
            let (settings, resolver) = (settings.clone(), resolver.clone());
            stuck.spawn(async move { lookup(&settings, &resolver, &visit("hang.test:22")).await });
        }
        while let Some(r) = stuck.join_next().await {
            assert_eq!(r.unwrap().unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        }
        let started = Instant::now();
        let refused = lookup(&settings, &resolver, &visit("hang.test:22")).await;
        assert!(refused.unwrap_err().to_string().contains("still running"));
        assert!(started.elapsed() < Duration::from_millis(500));
        let mut patient = settings.clone();
        patient.connect_timeout = Duration::from_secs(5);
        assert!(
            lookup(&patient, &resolver, &visit("127.0.0.1:22"))
                .await
                .is_ok()
        );
        let mut service = visit("localhost:22");
        service.service = "ssh".into();
        assert!(lookup(&patient, &resolver, &service).await.is_ok());
        let mut hanging = visit("hang.test:22");
        hanging.service = "ssh".into();
        let mut stuck = tokio::task::JoinSet::new();
        for _ in 0..MAX_SERVICE_LOOKUPS {
            let (settings, resolver, hanging) =
                (settings.clone(), resolver.clone(), hanging.clone());
            stuck.spawn(async move { lookup(&settings, &resolver, &hanging).await });
        }
        while let Some(r) = stuck.join_next().await {
            assert_eq!(r.unwrap().unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        }
        let refused = lookup(&settings, &resolver, &hanging).await;
        assert!(refused.unwrap_err().to_string().contains("for services"));
    }
}
