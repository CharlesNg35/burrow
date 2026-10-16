use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::allowlist::AllowList;
use crate::config::check::parse_host_port;
use crate::config::model::Protocol;
use crate::config::settings::ClientSettings;
use crate::failure::io_reason;
use crate::forward::{client_tcp, refuse};
use crate::handshake::Channel;
use crate::protocol::{
    Connect, KEEPALIVE_RECORD, ServiceInfo, Status, decode_datagram, encode_datagram,
};
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

async fn connected_socket(target: SocketAddr) -> Option<Arc<UdpSocket>> {
    let bind: SocketAddr = if target.is_ipv4() {
        (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
    };
    let sock = UdpSocket::bind(bind).await.ok()?;
    sock.connect(target).await.ok()?;
    Some(Arc::new(sock))
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
    let mut buf = vec![0u8; 65536];
    let check = timeout.min(Duration::from_secs(5));
    loop {
        match tokio::time::timeout(check, sock.recv(&mut buf)).await {
            Ok(Ok(n)) => {
                seen.store(epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
                if tx.send((id, buf[..n].to_vec())).await.is_err() {
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
    let Ok((host, port)) = parse_host_port(&svc.target) else {
        return;
    };
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        let addr = SocketAddr::new(ip, port);
        if !settings.allow.permits(addr) {
            warn!(
                "service \"{}\" targets {addr}, which the allow list does not permit; add \"{}\" to \"allow\" to expose it",
                svc.name,
                AllowList::suggestion(addr)
            );
        }
    }
}

pub async fn serve(settings: Arc<ClientSettings>, resolver: Arc<dyn Resolver>, mut ch: Channel) {
    let idle_limit = ch.stream.is_websocket().then_some(IDLE_CHANNEL_LIMIT);
    let first = loop {
        let next = ch.reader.recv(&mut ch.stream);
        let rec = match idle_limit {
            Some(limit) => match tokio::time::timeout(limit, next).await {
                Ok(r) => r,
                Err(_) => return,
            },
            None => next.await,
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
            refuse(ch, Status::Invalid).await;
            return;
        }
    };
    let addrs = match permitted(&settings, &*resolver, &connect).await {
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
                    warn!(
                        "service \"{}\": cannot connect to {} ({}); is it running?",
                        connect.service,
                        connect.target,
                        io_reason(&e)
                    );
                    refuse(ch, Status::Unreachable).await;
                    return;
                }
            };
            tune(&target, &settings.tcp);
            if let Err(e) = client_tcp(ch, target, settings.insecure_plaintext).await {
                debug!("service \"{}\": connection ended: {e:?}", connect.service);
            }
        }
        Protocol::Udp => udp(settings, ch, connect, addrs).await,
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
    let resolved = match resolver.resolve(&connect.target).await {
        Ok(a) => a,
        Err(e) => {
            warn!(
                "service \"{}\": cannot resolve {} ({})",
                connect.service,
                connect.target,
                io_reason(&e)
            );
            return Err(Status::Unresolved);
        }
    };
    let allowed: Vec<SocketAddr> = resolved
        .iter()
        .copied()
        .filter(|a| settings.allow.permits(*a))
        .collect();
    if allowed.is_empty() {
        let first = resolved[0];
        warn!(
            "denied connection to {} for service \"{}\": {first} is not in the allow list; add \"{}\" to \"allow\" to permit it",
            connect.target,
            connect.service,
            AllowList::suggestion(first)
        );
        return Err(Status::Denied);
    }
    Ok(allowed)
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
                    continue;
                }
                let Some(sock) = connected_socket(target).await else {
                    continue;
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
