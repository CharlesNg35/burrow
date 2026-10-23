use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::tcp::log_status;
use super::{Shared, TakeError};
use crate::config::settings::Service;
use crate::forward::ForwardError;
use crate::protocol::{Connect, Status, decode_datagram, encode_datagram};
use burrow_config::Protocol;

const MAX_DATAGRAM: usize = crate::noise::MAX_PLAINTEXT - 4;

#[derive(Default)]
struct Peers {
    by_addr: HashMap<SocketAddr, (u32, Instant)>,
    by_id: HashMap<u32, SocketAddr>,
    next: u32,
}

impl Peers {
    fn id_for(&mut self, addr: SocketAddr, most: impl FnOnce() -> usize) -> Option<u32> {
        if let Some((id, seen)) = self.by_addr.get_mut(&addr) {
            *seen = Instant::now();
            return Some(*id);
        }
        if self.by_addr.len() >= most() {
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
        self.retain_where(|_, seen| seen.elapsed() < timeout);
    }

    fn retain(&mut self, keep: impl Fn(SocketAddr) -> bool) {
        self.retain_where(|addr, _| keep(addr));
    }

    fn retain_where(&mut self, keep: impl Fn(SocketAddr, Instant) -> bool) {
        let by_id = &mut self.by_id;
        self.by_addr.retain(|addr, (id, seen)| {
            let kept = keep(*addr, *seen);
            if !kept {
                by_id.remove(id);
            }
            kept
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
    index: u16,
) {
    let socket = Arc::new(socket);
    let mut stop = shared.shutdown.clone();
    loop {
        if *stop.borrow() {
            return;
        }
        let svc = config.borrow().clone();
        let target = svc.target_at(index);
        let Some(session) = shared.pick(&name, &svc, &target) else {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
                _ = stop.changed() => return,
            }
        };
        let ch = match tokio::time::timeout(Duration::from_secs(10), session.take()).await {
            Ok(Ok(ch)) => ch,
            Ok(Err(TakeError::TooManyPending)) | Ok(Err(TakeError::Closed)) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            Err(_) => {
                if svc.clients.len() > 1 {
                    shared.troubled(&name, &session, &target, &crate::forward::no_reply());
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        debug!(
            "service \"{name}\": UDP relay attached to client \"{}\"",
            session.name()
        );
        let port = Port {
            name: &name,
            svc: &svc,
            target: &target,
        };
        match relay(&shared, port, &session, socket.clone(), ch, config.clone()).await {
            Ok(()) => {}
            Err(Some(status)) => {
                if status == Status::Busy {
                    shared.metrics.client_busy(session.name());
                }
                log_status(
                    &format!("service \"{name}\""),
                    session.name(),
                    &target,
                    status,
                );
                if svc.clients.len() > 1 {
                    shared.troubled(&name, &session, &target, &ForwardError::Status(status));
                }
                let pause = if svc.clients.len() > 1 && shared.has_clear(&name, &svc, &target) {
                    Duration::from_millis(200)
                } else {
                    Duration::from_secs(5)
                };
                tokio::time::sleep(pause).await;
            }
            Err(None) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}

struct Port<'a> {
    name: &'a str,
    svc: &'a Service,
    target: &'a str,
}

async fn relay(
    shared: &Arc<Shared>,
    port: Port<'_>,
    session: &super::via::Via,
    socket: Arc<UdpSocket>,
    ch: crate::handshake::Channel,
    mut config: watch::Receiver<Service>,
) -> Result<(), Option<Status>> {
    let Port { name, svc, target } = port;
    let crate::handshake::Channel {
        stream,
        mut writer,
        mut reader,
    } = ch;
    let (mut rd, mut wr) = stream.into_split();
    let connect = Connect {
        protocol: Protocol::Udp,
        service: name.to_string(),
        target: target.to_string(),
        only: None,
    };
    writer
        .send(&mut wr, &connect.encode())
        .await
        .map_err(|_| None)?;
    let wait = shared.settings().heartbeat_interval * 3;
    let status = match tokio::time::timeout(wait, reader.recv(&mut rd)).await {
        Ok(Ok(Some(b))) => Status::decode(b).map_err(|_| None)?,
        Ok(_) => return Err(None),
        Err(_) => {
            static SILENT: crate::throttle::Throttle = crate::throttle::Throttle::new();
            if let Some(more) = SILENT.allow() {
                warn!(
                    "service \"{name}\": client \"{}\" did not answer the UDP relay within {}s; trying again{more}",
                    session.name(),
                    wait.as_secs()
                );
            }
            if svc.clients.len() > 1 {
                shared.troubled(name, session, target, &crate::forward::no_reply());
            }
            return Err(None);
        }
    };
    if status != Status::Ok {
        return Err(Some(status));
    }
    if svc.clients.len() > 1 {
        shared.accepted(name, session.name(), target);
    }
    info!(
        "service \"{name}\": UDP relay ready via client \"{}\"",
        session.name()
    );

    let peers = Arc::new(Mutex::new(Peers::default()));
    let back_peers = peers.clone();
    let back_socket = socket.clone();
    let limit = shared.limit(name);
    let back_limit = limit.clone();
    let stats = shared.metrics.service(name);
    let back_stats = stats.clone();
    let back = tokio::spawn(async move {
        loop {
            match reader.recv(&mut rd).await {
                Ok(Some(rec)) => {
                    let Ok((id, payload)) = decode_datagram(rec) else {
                        return;
                    };
                    if !back_limit.outbound.try_take(payload.len()) {
                        continue;
                    }
                    let addr = back_peers.lock().expect("peers lock").touch(id);
                    if let Some(addr) = addr {
                        back_stats
                            .traffic
                            .outbound
                            .fetch_add(payload.len() as u64, std::sync::atomic::Ordering::Relaxed);
                        let _ = back_socket.send_to(payload, addr).await;
                    }
                }
                _ => return,
            }
        }
    });

    let mut back = AbortOnDrop(back);
    let mut gauge = PeerGauge {
        shared: shared.clone(),
        counted: 0,
    };
    let mut sweep = tokio::time::interval(Duration::from_secs(5));
    let mut repick = tokio::time::interval(Duration::from_secs(1));
    let mut buf = vec![0u8; 65536];
    let mut rec = Vec::with_capacity(65536);
    let mut stop = shared.shutdown.clone();
    let stall = shared.settings().heartbeat_interval * 3;
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
                if n > MAX_DATAGRAM || !config.borrow().sources.permits(from.ip()) {
                    continue;
                }
                if !limit.inbound.try_take(n) {
                    continue;
                }
                stats
                    .traffic
                    .inbound
                    .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                let (id, count, before) = {
                    let mut p = peers.lock().expect("peers lock");
                    let before = p.len();
                    (
                        p.id_for(from, || shared.settings().udp_max_peers),
                        p.len(),
                        before,
                    )
                };
                if count > before {
                    stats.total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                gauge.set(count);
                let Some(id) = id else {
                    static FULL: crate::throttle::Throttle = crate::throttle::Throttle::new();
                    if let Some(more) = FULL.allow() {
                        warn!(
                            "service \"{name}\": udp.max_peers {} reached, dropping datagram from {from}; raise udp.max_peers in server.yaml{more}",
                            shared.settings().udp_max_peers
                        );
                    }
                    continue;
                };
                encode_datagram(id, &buf[..n], &mut rec);
                let sent = tokio::select! {
                    r = tokio::time::timeout(stall, writer.send(&mut wr, &rec)) => r,
                    _ = session.closed() => break Err(None),
                };
                match sent {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => break Err(None),
                    Err(_) => {
                        static STALLED: crate::throttle::Throttle = crate::throttle::Throttle::new();
                        if let Some(more) = STALLED.allow() {
                            warn!(
                                "service \"{name}\": client \"{}\" stopped reading its UDP relay for {}s; trying again{more}",
                                session.name(),
                                stall.as_secs()
                            );
                        }
                        if svc.clients.len() > 1 {
                            shared.troubled(name, session, target, &crate::forward::no_reply());
                        }
                        break Err(None);
                    }
                }
            }
            _ = sweep.tick() => {
                let count = {
                    let mut p = peers.lock().expect("peers lock");
                    p.expire(shared.settings().udp_session_timeout);
                    p.len()
                };
                gauge.set(count);
            }
            _ = repick.tick() => {
                let current = config.borrow().clone();
                if let Some(preferred) = shared
                    .preferred(name, &current, target)
                    .filter(|p| p != session.name())
                {
                    debug!("service \"{name}\": moving the UDP relay to client \"{preferred}\"");
                    break Ok(());
                }
            }
            _ = &mut back.0 => break Err(None),
            r = config.changed() => {
                let moved = {
                    let next = config.borrow();
                    next.target != svc.target || !next.clients.iter().any(|c| c == session.name())
                };
                if r.is_err() || moved {
                    break Ok(());
                }
                let sources = config.borrow().sources.clone();
                let count = {
                    let mut p = peers.lock().expect("peers lock");
                    p.retain(|addr| sources.permits(addr.ip()));
                    p.len()
                };
                gauge.set(count);
            }
            _ = session.closed() => break Ok(()),
            _ = stop.changed() => break Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn the_peer_cap_is_read_for_each_new_peer_and_never_drops_known_ones() {
        let mut p = Peers::default();
        assert!(p.id_for(at(1), || 2).is_some());
        assert!(p.id_for(at(2), || 2).is_some());
        assert!(p.id_for(at(3), || 2).is_none());
        assert!(p.id_for(at(3), || 3).is_some());
        assert!(p.id_for(at(4), || 1).is_none());
        for known in 1..=3 {
            assert!(p.id_for(at(known), || 1).is_some());
        }
        assert_eq!(p.len(), 3);
        p.expire(Duration::ZERO);
        assert_eq!(p.len(), 0);
        assert!(p.id_for(at(5), || 1).is_some());
        assert!(p.id_for(at(6), || 1).is_none());
    }
}
