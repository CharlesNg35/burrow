use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::tcp::log_status;
use super::{Shared, TakeError};
use crate::config::settings::Service;
use crate::forward::ForwardError;
use crate::protocol::{Connect, MAX_DATAGRAM, Status, decode_datagram, encode_datagram};
use crate::udp::link::{Link, Opening, open_udp};
use crate::udp::peers::{AbortOnDrop, PeerCount, PeerGauge, PeerTable};
use burrow_config::Protocol;

impl PeerCount for Arc<Shared> {
    fn peers(&self) -> &std::sync::atomic::AtomicUsize {
        &self.udp_peers
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
    let connect = Connect {
        protocol: Protocol::Udp,
        service: name.to_string(),
        target: target.to_string(),
        only: None,
    };
    let wait = shared.settings().heartbeat_interval * 3;
    let Link {
        mut reader,
        mut rd,
        mut writer,
        mut wr,
    } = match open_udp(ch, &connect.encode(), wait).await {
        Ok(link) => link,
        Err(Opening::Lost) => return Err(None),
        Err(Opening::Refused(status)) => return Err(Some(status)),
        Err(Opening::Silent) => {
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
    if svc.clients.len() > 1 {
        shared.accepted(name, session.name(), target);
    }
    info!(
        "service \"{name}\": UDP relay ready via client \"{}\"",
        session.name()
    );

    let peers = Arc::new(Mutex::new(PeerTable::default()));
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
    let mut gauge = PeerGauge::new(shared.clone());
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
