use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::tcp::log_status;
use super::{Shared, TakeError};
use crate::config::model::Protocol;
use crate::config::settings::Service;
use crate::protocol::{Connect, Status, decode_datagram, encode_datagram};

pub const MAX_PEERS: usize = 4096;
const MAX_DATAGRAM: usize = crate::noise::MAX_PLAINTEXT - 4;

#[derive(Default)]
struct Peers {
    by_addr: HashMap<SocketAddr, (u32, Instant)>,
    by_id: HashMap<u32, SocketAddr>,
    next: u32,
}

impl Peers {
    fn id_for(&mut self, addr: SocketAddr) -> Option<u32> {
        if let Some((id, seen)) = self.by_addr.get_mut(&addr) {
            *seen = Instant::now();
            return Some(*id);
        }
        if self.by_addr.len() >= MAX_PEERS {
            return None;
        }
        self.next = self.next.wrapping_add(1);
        while self.by_id.contains_key(&self.next) {
            self.next = self.next.wrapping_add(1);
        }
        let id = self.next;
        self.by_addr.insert(addr, (id, Instant::now()));
        self.by_id.insert(id, addr);
        Some(id)
    }

    fn touch(&mut self, id: u32) -> Option<SocketAddr> {
        let addr = *self.by_id.get(&id)?;
        if let Some((_, seen)) = self.by_addr.get_mut(&addr) {
            *seen = Instant::now();
        }
        Some(addr)
    }

    fn len(&self) -> usize {
        self.by_addr.len()
    }

    fn expire(&mut self, timeout: Duration) {
        let by_id = &mut self.by_id;
        self.by_addr.retain(|_, (id, seen)| {
            let keep = seen.elapsed() < timeout;
            if !keep {
                by_id.remove(id);
            }
            keep
        });
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct PeerGauge {
    shared: Arc<Shared>,
    counted: usize,
}

impl PeerGauge {
    fn set(&mut self, now: usize) {
        if now > self.counted {
            self.shared
                .udp_peers
                .fetch_add(now - self.counted, std::sync::atomic::Ordering::AcqRel);
        } else {
            self.shared
                .udp_peers
                .fetch_sub(self.counted - now, std::sync::atomic::Ordering::AcqRel);
        }
        self.counted = now;
    }
}

impl Drop for PeerGauge {
    fn drop(&mut self) {
        self.set(0);
    }
}

pub async fn udp_service(
    shared: Arc<Shared>,
    name: String,
    socket: UdpSocket,
    config: watch::Receiver<Service>,
) {
    let socket = Arc::new(socket);
    let mut stop = shared.shutdown.clone();
    loop {
        if *stop.borrow() {
            return;
        }
        let svc = config.borrow().clone();
        let Some(session) = shared.session(&svc.client) else {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
                _ = stop.changed() => return,
            }
        };
        let ch = match tokio::time::timeout(Duration::from_secs(10), session.take()).await {
            Ok(Ok(ch)) => ch,
            Ok(Err(TakeError::TooManyPending)) | Ok(Err(TakeError::Closed)) | Err(_) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let (_active, _) = session.connection_guard();
        debug!(
            "service \"{name}\": UDP relay attached to client \"{}\"",
            svc.client
        );
        match relay(
            &shared,
            &name,
            &svc,
            &session,
            socket.clone(),
            ch,
            config.clone(),
        )
        .await
        {
            Ok(()) => {}
            Err(Some(status)) => {
                log_status(
                    &format!("service \"{name}\""),
                    &svc.client,
                    &svc.target,
                    status,
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Err(None) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}

async fn relay(
    shared: &Arc<Shared>,
    name: &str,
    svc: &Service,
    session: &super::ClientSession,
    socket: Arc<UdpSocket>,
    ch: crate::handshake::Channel,
    mut config: watch::Receiver<Service>,
) -> Result<(), Option<Status>> {
    let crate::handshake::Channel {
        stream,
        mut writer,
        mut reader,
    } = ch;
    let (mut rd, mut wr) = stream.into_split();
    let connect = Connect {
        protocol: Protocol::Udp,
        service: name.to_string(),
        target: svc.target.clone(),
    };
    writer
        .send(&mut wr, &connect.encode())
        .await
        .map_err(|_| None)?;
    let status = match reader.recv(&mut rd).await {
        Ok(Some(b)) => Status::decode(b).map_err(|_| None)?,
        _ => return Err(None),
    };
    if status != Status::Ok {
        return Err(Some(status));
    }
    info!(
        "service \"{name}\": UDP relay ready via client \"{}\"",
        svc.client
    );

    let peers = Arc::new(Mutex::new(Peers::default()));
    let back_peers = peers.clone();
    let back_socket = socket.clone();
    let back = tokio::spawn(async move {
        loop {
            match reader.recv(&mut rd).await {
                Ok(Some(rec)) => {
                    let Ok((id, payload)) = decode_datagram(rec) else {
                        return;
                    };
                    let addr = back_peers.lock().expect("peers lock").touch(id);
                    if let Some(addr) = addr {
                        let _ = back_socket.send_to(payload, addr).await;
                    }
                }
                _ => return,
            }
        }
    });

    let mut back = AbortOnDrop(back);
    let timeout = shared.settings().udp_session_timeout;
    let mut gauge = PeerGauge {
        shared: shared.clone(),
        counted: 0,
    };
    let mut sweep = tokio::time::interval(Duration::from_secs(5));
    let mut buf = vec![0u8; 65536];
    let mut rec = Vec::with_capacity(65536);
    let mut stop = shared.shutdown.clone();
    loop {
        tokio::select! {
            r = socket.recv_from(&mut buf) => {
                let (n, from) = match r {
                    Ok(v) => v,
                    Err(e) => {
                        debug!("service \"{name}\": UDP receive error: {e}");
                        continue;
                    }
                };
                if n > MAX_DATAGRAM {
                    continue;
                }
                let (id, count) = {
                    let mut p = peers.lock().expect("peers lock");
                    (p.id_for(from), p.len())
                };
                gauge.set(count);
                let Some(id) = id else {
                    static FULL: crate::throttle::Throttle = crate::throttle::Throttle::new();
                    if let Some(more) = FULL.allow() {
                        warn!("service \"{name}\": too many UDP peers ({MAX_PEERS}), dropping datagram from {from}{more}");
                    }
                    continue;
                };
                encode_datagram(id, &buf[..n], &mut rec);
                if writer.send(&mut wr, &rec).await.is_err() {
                    break Err(None);
                }
            }
            _ = sweep.tick() => {
                let count = {
                    let mut p = peers.lock().expect("peers lock");
                    p.expire(timeout);
                    p.len()
                };
                gauge.set(count);
            }
            _ = &mut back.0 => break Err(None),
            r = config.changed() => {
                if r.is_err() || config.borrow().target != svc.target || config.borrow().client != svc.client {
                    break Ok(());
                }
            }
            _ = session.closed() => break Ok(()),
            _ = stop.changed() => break Ok(()),
        }
    }
}
