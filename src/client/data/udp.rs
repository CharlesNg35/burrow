use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::limits::Slot;
use super::{Dials, Rules, permitted};
use crate::base::failure::io_reason;
use crate::policy::throttle::Throttle;
use crate::wire::protocol::{Connect, MAX_DATAGRAM, Status, decode_datagram, encode_datagram};
use crate::wire::transport::Channel;
use crate::wire::transport::resolve::Resolver;

struct Peer {
    sock: Arc<UdpSocket>,
    to: SocketAddr,
    seen: Arc<AtomicU64>,
    task: tokio::task::AbortHandle,
    _slot: Slot,
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

fn receive(sock: &UdpSocket) -> Option<Vec<u8>> {
    match crate::wire::transport::udp::try_recv_owned(sock) {
        Ok((datagram, _)) => Some(datagram),
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
}

pub(super) fn preferred(addrs: &[SocketAddr]) -> SocketAddr {
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

fn resolve_again<W: Dials>(
    live: &Rules<W>,
    resolver: &Arc<dyn Resolver>,
    connect: &Connect,
    resolved: &Arc<Mutex<(SocketAddr, Instant)>>,
    resolving: &Arc<std::sync::atomic::AtomicBool>,
) {
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

fn existing_peer(
    peers: &Mutex<HashMap<u32, Peer>>,
    id: u32,
    target: SocketAddr,
    epoch: Instant,
) -> Option<(Arc<UdpSocket>, Arc<AtomicU64>)> {
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
}

fn warn_peers_full<W: Dials>(live: &Rules<W>) {
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
}

pub(super) async fn udp<W: Dials>(
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
        let mut sealed = Vec::new();
        while let Some((id, data)) = rx.recv().await {
            encode_datagram(id, &data, &mut rec);
            if writer.send_with(&mut wr, &rec, &mut sealed).await.is_err() {
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
            resolve_again(&live, &resolver, &connect, &resolved, &resolving);
        }
        let existing = existing_peer(&peers, id, target, epoch);
        let (sock, seen) = match existing {
            Some(p) => p,
            None => {
                let Some(slot) = live.1.peers.take() else {
                    warn_peers_full(&live);
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

pub(super) fn unreachable_status(e: &std::io::Error) -> Status {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => Status::Offline,
        _ => Status::Unreachable,
    }
}

pub(super) fn unreachable_hint(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::AddrInUse => {
            "this machine has no local port left for it, usually because many recent connections to it are still closing (TIME_WAIT)"
        }
        _ => "is it running?",
    }
}
