use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::{Shared, TakeError, log_status};
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
        let id = self.next;
        self.by_addr.insert(addr, (id, Instant::now()));
        self.by_id.insert(id, addr);
        Some(id)
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
        let _active = session.connection_guard();
        debug!(
            "service \"{name}\": UDP relay attached to client \"{}\"",
            svc.client
        );
        match relay(&shared, &name, &svc, socket.clone(), ch, config.clone()).await {
            Ok(()) => {}
            Err(Some(status)) => {
                log_status(&name, &svc, status);
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
    let mut back = tokio::spawn(async move {
        loop {
            match reader.recv(&mut rd).await {
                Ok(Some(rec)) => {
                    let Ok((id, payload)) = decode_datagram(rec) else {
                        return;
                    };
                    let addr = back_peers
                        .lock()
                        .expect("peers lock")
                        .by_id
                        .get(&id)
                        .copied();
                    if let Some(addr) = addr {
                        let _ = back_socket.send_to(payload, addr).await;
                    }
                }
                _ => return,
            }
        }
    });

    let timeout = shared.settings().udp_session_timeout;
    let mut sweep = tokio::time::interval(Duration::from_secs(5));
    let mut buf = vec![0u8; 65536];
    let mut rec = Vec::with_capacity(65536);
    let mut stop = shared.shutdown.clone();
    let result = loop {
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
                let Some(id) = peers.lock().expect("peers lock").id_for(from) else {
                    warn!("service \"{name}\": too many UDP peers ({MAX_PEERS}), dropping datagram from {from}");
                    continue;
                };
                encode_datagram(id, &buf[..n], &mut rec);
                if writer.send(&mut wr, &rec).await.is_err() {
                    break Err(None);
                }
            }
            _ = sweep.tick() => peers.lock().expect("peers lock").expire(timeout),
            _ = &mut back => break Err(None),
            r = config.changed() => {
                if r.is_err() || config.borrow().target != svc.target || config.borrow().client != svc.client {
                    break Ok(());
                }
            }
            _ = stop.changed() => break Ok(()),
        }
    };
    back.abort();
    result
}
