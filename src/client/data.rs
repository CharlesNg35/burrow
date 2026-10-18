use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, warn};

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
        sock.try_recv(&mut buf).ok().map(|n| buf[..n].to_vec())
    })
}

async fn peer_replies(
    id: u32,
    sock: Arc<UdpSocket>,
    seen: Arc<AtomicU64>,
    epoch: Instant,
    timeout: Duration,
    tx: mpsc::Sender<(u32, Vec<u8>)>,
    peers: Arc<Mutex<HashMap<u32, Peer>>>,
) {
    let check = timeout.min(Duration::from_secs(5));
    loop {
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
                let idle = epoch
                    .elapsed()
                    .saturating_sub(Duration::from_millis(seen.load(Ordering::Relaxed)));
                if idle >= timeout {
                    break;
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
    let connect = match first {
        Ok(c) => c,
        Err(_) => {
            reply(ch, Status::Invalid).await;
            return;
        }
    };
    let settings = live.settings();
    if let Some(path) = unix::path_of(&connect.target) {
        socket(&settings, ch, &connect, path).await;
        return;
    }
    let addrs = match permitted(&settings, &*resolver, &connect).await {
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
            if let Err(e) = client_tcp(ch, target, settings.insecure_plaintext).await {
                debug!("{}: connection ended: {e:?}", label(&connect));
            }
        }
        Protocol::Udp => udp(settings, ch, connect, addrs).await,
    }
}

async fn socket(settings: &ClientSettings, ch: Channel, connect: &Connect, path: &str) {
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
    match tokio::net::UnixStream::connect(path).await {
        Ok(target) => {
            if let Err(e) = client_tcp(ch, target, settings.insecure_plaintext).await {
                debug!("{}: connection ended: {e:?}", label(connect));
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

fn whom(connect: &Connect) -> String {
    match connect.service.as_str() {
        "" => "a visitor".to_string(),
        s => format!("service \"{s}\""),
    }
}

async fn permitted(
    settings: &ClientSettings,
    resolver: &dyn Resolver,
    connect: &Connect,
) -> Result<Vec<SocketAddr>, Status> {
    if parse_host_port(&connect.target).is_err() {
        return Err(Status::Invalid);
    }
    let lookup = tokio::time::timeout(settings.connect_timeout, resolver.resolve(&connect.target));
    let resolved = match lookup.await.unwrap_or_else(|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the lookup timed out",
        ))
    }) {
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

async fn udp(settings: Arc<ClientSettings>, ch: Channel, connect: Connect, addrs: Vec<SocketAddr>) {
    let Channel {
        stream,
        mut writer,
        mut reader,
    } = ch;
    let (mut rd, mut wr) = stream.into_split();
    let target = addrs[0];
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
    let timeout = settings.udp_session_timeout;
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
        let existing = peers
            .lock()
            .expect("peers lock")
            .get(&id)
            .map(|p| (p.sock.clone(), p.seen.clone()));
        let (sock, seen) = match existing {
            Some(p) => p,
            None => {
                if peers.lock().expect("peers lock").len() >= MAX_PEERS {
                    static FULL: Throttle = Throttle::new();
                    if let Some(more) = FULL.allow() {
                        warn!(
                            "UDP to {target}: {MAX_PEERS} peers are open, so a datagram from a new one was dropped; a udp.session_timeout_secs shorter than the server's frees them sooner{more}"
                        );
                    }
                    continue;
                }
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
                    timeout,
                    tx.clone(),
                    peers.clone(),
                ));
                peers.lock().expect("peers lock").insert(
                    id,
                    Peer {
                        sock: sock.clone(),
                        seen: seen.clone(),
                        task: task.abort_handle(),
                    },
                );
                (sock, seen)
            }
        };
        seen.store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        let _ = sock.send(payload).await;
    }
    debug!("service \"{}\": UDP relay closed", connect.service);
}
