use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::allowlist::AllowList;
use crate::config::check::{parse_host_port, unix};
use crate::config::model::Protocol;
use crate::config::settings::ClientSettings;
use crate::failure::io_reason;
use crate::forward::{client_tcp, reply};
use crate::handshake::Channel;
use crate::protocol::{
    Connect, KEEPALIVE_RECORD, ServiceInfo, Status, decode_datagram, encode_datagram,
};
use crate::throttle::Throttle;
use crate::transport::resolve::Resolver;
use crate::transport::tcp::{connect_any, tune};

const MAX_PEERS: usize = 4096;
const IDLE_CHANNEL_LIMIT: Duration = Duration::from_secs(90);

struct Peer {
    sock: Arc<UdpSocket>,
    seen: Arc<AtomicU64>,
    task: tokio::task::AbortHandle,
    _slot: Slot,
}

const MAX_OPEN_PEERS: usize = 8192;
static OPEN_PEERS: Slots = Slots::new(MAX_OPEN_PEERS);

const MAX_OPEN_CONNECTIONS: usize = 16384;
static OPEN_CONNECTIONS: Slots = Slots::new(MAX_OPEN_CONNECTIONS);

struct Slots {
    open: AtomicUsize,
    max: usize,
}

impl Slots {
    const fn new(max: usize) -> Slots {
        Slots {
            open: AtomicUsize::new(0),
            max,
        }
    }

    fn take(&'static self) -> Option<Slot> {
        self.open
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.max).then_some(n + 1)
            })
            .ok()
            .map(|_| Slot(self))
    }
}

struct Slot(&'static Slots);

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
    let sock = crate::transport::udp::bind_udp(bind).await?;
    sock.connect(target).await?;
    Ok(Arc::new(sock))
}

const MAX_REPLY: usize = crate::noise::MAX_PLAINTEXT - 4;

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

async fn peer_replies(
    id: u32,
    sock: Arc<UdpSocket>,
    seen: Arc<AtomicU64>,
    epoch: Instant,
    live: super::Live,
    tx: mpsc::Sender<(u32, Vec<u8>)>,
    peers: Arc<Mutex<HashMap<u32, Peer>>>,
) {
    loop {
        let timeout = live.settings().udp_session_timeout;
        let check = timeout.min(Duration::from_secs(5));
        match tokio::time::timeout(check, sock.readable()).await {
            Ok(Ok(())) => {
                let Some(datagram) = receive(&sock) else {
                    continue;
                };
                if datagram.len() > MAX_REPLY {
                    static LARGE: Throttle = Throttle::new();
                    if let Some(more) = LARGE.allow() {
                        warn!(
                            "UDP reply of {} bytes from {} dropped: the tunnel carries at most {MAX_REPLY} bytes per datagram{more}",
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

pub async fn serve(
    live: super::Live,
    resolver: Arc<dyn Resolver>,
    mut ch: Channel,
    mut ended: tokio::sync::watch::Receiver<bool>,
    held: super::Waiting,
) {
    let idle_limit = ch.stream.is_websocket().then_some(IDLE_CHANNEL_LIMIT);
    let first = loop {
        let next = ch.reader.recv(&mut ch.stream);
        let limited = async {
            match idle_limit {
                Some(limit) => tokio::time::timeout(limit, next).await.ok(),
                None => Some(next.await),
            }
        };
        let rec = tokio::select! {
            r = limited => match r {
                Some(r) => r,
                None => return,
            },
            _ = ended.wait_for(|e| *e) => return,
        };
        match rec {
            Ok(Some([KEEPALIVE_RECORD])) => continue,
            Ok(Some(b)) => break Connect::decode(b),
            _ => return,
        }
    };
    drop(held);
    let connect = match first {
        Ok(c) => c,
        Err(_) => {
            reply(ch, Status::Invalid).await;
            return;
        }
    };
    let Some(_slot) = OPEN_CONNECTIONS.take() else {
        static FULL: Throttle = Throttle::new();
        if let Some(more) = FULL.allow() {
            warn!(
                "{}: {MAX_OPEN_CONNECTIONS} connections are open through this client, so a new one was refused{more}",
                label(&connect)
            );
        }
        reply(ch, Status::Busy).await;
        return;
    };
    let settings = live.settings();
    if let Some(path) = unix::path_of(&connect.target) {
        socket(&live, &settings, ch, &connect, path).await;
        return;
    }
    let addrs = match permitted(&settings, &resolver, &connect).await {
        Ok(a) => a,
        Err(status) => {
            reply(ch, status).await;
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
                            "{}: cannot connect to {} ({}); is it running?{more}",
                            label(&connect),
                            connect.target,
                            io_reason(&e)
                        );
                    }
                    reply(ch, Status::Unreachable).await;
                    return;
                }
            };
            tune(&target, &settings.tcp);
            let Ok(peer) = target.peer_addr() else {
                return;
            };
            tokio::select! {
                r = client_tcp(ch, target, settings.insecure_plaintext) => {
                    if let Err(e) = r {
                        debug!("{}: connection ended: {e:?}", label(&connect));
                    }
                }
                _ = revoked(&live, |s| s.allow.permits(peer)) => {
                    no_longer_allowed(&connect);
                }
            }
        }
        Protocol::Udp => {
            let (watched, permitted) = (live.clone(), addrs.clone());
            let label = connect.clone();
            tokio::select! {
                _ = udp(live, resolver, ch, connect, addrs) => {}
                _ = revoked(&watched, |s| permitted.iter().any(|a| s.allow.permits(*a))) => {
                    no_longer_allowed(&label);
                }
            }
        }
    }
}

async fn revoked(live: &super::Live, allowed: impl Fn(&ClientSettings) -> bool) {
    let mut rx = live.0.subscribe();
    loop {
        if !allowed(&rx.borrow_and_update().settings) {
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

async fn socket(
    live: &super::Live,
    settings: &ClientSettings,
    ch: Channel,
    connect: &Connect,
    path: &str,
) {
    if connect.protocol != Protocol::Tcp || unix::check_path(path).is_err() {
        reply(ch, Status::Invalid).await;
        return;
    }
    if let Some(only) = connect.only.as_deref()
        && !AllowList::from_rules(only).permits_socket(path)
    {
        outside_reach(connect);
        reply(ch, Status::Denied).await;
        return;
    }
    if !settings.allow.permits_socket(path) {
        static DENIED: Throttle = Throttle::new();
        if let Some(more) = DENIED.allow() {
            warn!(
                "denied connection to {} for {}: it is not in the allow list; add \"{}\" to \"allow\" to permit it{more}",
                connect.target,
                whom(connect),
                connect.target
            );
        }
        reply(ch, Status::Denied).await;
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
        reply(ch, Status::Denied).await;
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
        reply(ch, Status::Denied).await;
        return;
    }
    #[cfg(unix)]
    match tokio::net::UnixStream::connect(path).await {
        Ok(target) => {
            tokio::select! {
                r = client_tcp(ch, target, settings.insecure_plaintext) => {
                    if let Err(e) = r {
                        debug!("{}: connection ended: {e:?}", label(connect));
                    }
                }
                _ = revoked(live, |s| s.allow.permits_socket(path)) => {
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
            reply(ch, Status::Unreachable).await;
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
        reply(ch, Status::Invalid).await;
    }
}

async fn lookup(
    settings: &ClientSettings,
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
    settings: &ClientSettings,
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
        .copied()
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
        .filter(|a| settings.allow.permits(*a))
        .collect();
    if allowed.is_empty() {
        static DENIED: Throttle = Throttle::new();
        if let Some(more) = DENIED.allow() {
            let first = resolved[0];
            let whom = whom(connect);
            warn!(
                "denied connection to {} for {whom}: {first} is not in the allow list; add \"{}\" to \"allow\" to permit it{more}",
                connect.target,
                AllowList::suggestion(first)
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

async fn udp(
    live: super::Live,
    resolver: Arc<dyn Resolver>,
    ch: Channel,
    connect: Connect,
    addrs: Vec<SocketAddr>,
) {
    let Channel {
        stream,
        mut writer,
        mut reader,
    } = ch;
    let (mut rd, mut wr) = stream.into_split();
    let resolved = Arc::new(Mutex::new((preferred(&addrs), Instant::now())));
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
        let existing = peers.lock().expect("peers lock").get(&id).map(|p| {
            p.seen
                .store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
            (p.sock.clone(), p.seen.clone())
        });
        let (sock, seen) = match existing {
            Some(p) => p,
            None => {
                let (target, at) = *resolved.lock().expect("resolved lock");
                if at.elapsed() >= RESOLVE_AGAIN && !resolving.swap(true, Ordering::AcqRel) {
                    let (live, resolver, connect) =
                        (live.clone(), resolver.clone(), connect.clone());
                    let (resolved, resolving) = (resolved.clone(), resolving.clone());
                    tokio::spawn(async move {
                        let found = permitted(&live.settings(), &resolver, &connect).await;
                        let mut current = resolved.lock().expect("resolved lock");
                        *current = match found {
                            Ok(addrs) => (preferred(&addrs), Instant::now()),
                            Err(_) => (current.0, Instant::now()),
                        };
                        resolving.store(false, Ordering::Release);
                    });
                }
                if peers.lock().expect("peers lock").len() >= MAX_PEERS {
                    static FULL: Throttle = Throttle::new();
                    if let Some(more) = FULL.allow() {
                        warn!(
                            "UDP to {target}: {MAX_PEERS} peers are open, so a datagram from a new one was dropped; a udp.session_timeout_secs shorter than the server's frees them sooner{more}"
                        );
                    }
                    continue;
                }
                let Some(slot) = OPEN_PEERS.take() else {
                    static ALL: Throttle = Throttle::new();
                    if let Some(more) = ALL.allow() {
                        warn!(
                            "UDP to {target}: {MAX_OPEN_PEERS} peers are open across all UDP services, so a datagram from a new one was dropped{more}"
                        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_peers_across_all_services_share_one_limit() {
        let slots: &'static Slots = Box::leak(Box::new(Slots::new(2)));
        let first = slots.take().unwrap();
        let _second = slots.take().unwrap();
        assert!(slots.take().is_none());
        drop(first);
        assert!(slots.take().is_some());
    }
    use crate::transport::resolve::Resolved;

    struct Hanging;

    impl Resolver for Hanging {
        fn resolve<'a>(&'a self, host_port: &'a str) -> Resolved<'a> {
            Box::pin(async move {
                if host_port.starts_with("hang.test:") {
                    std::future::pending::<()>().await;
                }
                crate::transport::resolve::resolve(host_port).await
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
