use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
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
use crate::net::{connect_any, resolve, tune};
use crate::protocol::{Connect, ServiceInfo, Status, decode_datagram, encode_datagram};

const MAX_PEERS: usize = 4096;

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

pub async fn serve(settings: Arc<ClientSettings>, mut ch: Channel) {
    let first = match ch.reader.recv(&mut ch.stream).await {
        Ok(Some(b)) => Connect::decode(b),
        _ => return,
    };
    let connect = match first {
        Ok(c) => c,
        Err(_) => {
            refuse(ch, Status::Invalid).await;
            return;
        }
    };
    let addrs = match permitted(&settings, &connect).await {
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
    connect: &Connect,
) -> Result<Vec<SocketAddr>, Status> {
    if parse_host_port(&connect.target).is_err() {
        return Err(Status::Invalid);
    }
    let resolved = match resolve(&connect.target).await {
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
    let mut peers: HashMap<u32, (Arc<UdpSocket>, Instant, tokio::task::JoinHandle<()>)> =
        HashMap::new();
    let mut last_sweep = Instant::now();
    loop {
        let rec = match reader.recv(&mut rd).await {
            Ok(Some(r)) => r,
            _ => break,
        };
        let Ok((id, payload)) = decode_datagram(rec) else {
            break;
        };
        if last_sweep.elapsed() > Duration::from_secs(5) {
            last_sweep = Instant::now();
            peers.retain(|_, (_, seen, task)| {
                let keep = seen.elapsed() < timeout && !task.is_finished();
                if !keep {
                    task.abort();
                }
                keep
            });
        }
        if !peers.contains_key(&id) {
            if peers.len() >= MAX_PEERS {
                continue;
            }
            let bind: SocketAddr = if target.is_ipv4() {
                "0.0.0.0:0".parse().expect("static addr")
            } else {
                "[::]:0".parse().expect("static addr")
            };
            let Ok(sock) = UdpSocket::bind(bind).await else {
                continue;
            };
            if sock.connect(target).await.is_err() {
                continue;
            }
            let sock = Arc::new(sock);
            let recv_sock = sock.clone();
            let tx = tx.clone();
            let task = tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                loop {
                    match tokio::time::timeout(timeout, recv_sock.recv(&mut buf)).await {
                        Ok(Ok(n)) => {
                            if tx.send((id, buf[..n].to_vec())).await.is_err() {
                                return;
                            }
                        }
                        Ok(Err(_)) => continue,
                        Err(_) => return,
                    }
                }
            });
            peers.insert(id, (sock, Instant::now(), task));
        }
        if let Some((sock, seen, _)) = peers.get_mut(&id) {
            *seen = Instant::now();
            let _ = sock.send(payload).await;
        }
    }
    for (_, (_, _, t)) in peers {
        t.abort();
    }
    writer_task.abort();
    debug!("service \"{}\": UDP relay closed", connect.service);
}
