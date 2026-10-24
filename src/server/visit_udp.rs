use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use super::Shared;
use super::route::{Outcome, Tries, Way};
use super::via::Via;
use super::visit::Grant;
use crate::forward::{ForwardError, Opened};
use crate::metrics::ServiceStats;
use crate::protocol::{Connect, Status, VisitRequest, VisitTarget, decode_datagram};
use crate::ratelimit::Limit;
use crate::secret::Secret;
use crate::throttle::Throttle;
use crate::transport::Channel;
use crate::udp::link::{Link, Opening, open_udp};
use crate::udp::peers::{PeerGauge, PeerTable};
use burrow_config::Protocol;

const SWEEP: Duration = Duration::from_secs(5);

pub(super) struct Visit<'a> {
    pub visitor: &'a str,
    pub secret: &'a Secret,
    pub request: &'a VisitRequest,
    pub peer: SocketAddr,
    pub grant: &'a Grant,
    pub label: &'a str,
    pub several: bool,
}

struct Peers {
    table: PeerTable<u32>,
    gauge: PeerGauge<Arc<Shared>>,
    last: Instant,
}

struct Meter {
    limit: Option<Arc<Limit>>,
    stats: Option<Arc<ServiceStats>>,
}

impl Meter {
    fn take_in(&self, n: usize) -> bool {
        self.limit.as_ref().is_none_or(|l| l.inbound.try_take(n))
    }

    fn take_out(&self, n: usize) -> bool {
        self.limit.as_ref().is_none_or(|l| l.outbound.try_take(n))
    }

    fn count_in(&self, n: usize) {
        if let Some(s) = &self.stats {
            s.traffic.inbound.fetch_add(n as u64, Ordering::Relaxed);
        }
    }

    fn count_out(&self, n: usize) {
        if let Some(s) = &self.stats {
            s.traffic.outbound.fetch_add(n as u64, Ordering::Relaxed);
        }
    }

    fn new_peer(&self) {
        if let Some(s) = &self.stats {
            s.total.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub(super) async fn serve(
    shared: &Arc<Shared>,
    channel: &mut Channel,
    visit: Visit<'_>,
    clients: Vec<Via>,
) -> (Outcome, bool) {
    let grant = visit.grant;
    let way = Way {
        service: grant.service.as_deref(),
        target: &grant.target,
        label: visit.label,
        peer: visit.peer,
        several: visit.several,
    };
    let connect = Connect {
        protocol: Protocol::Udp,
        service: grant.service.clone().unwrap_or_default(),
        target: grant.target.clone(),
        only: grant.only.clone(),
    }
    .encode();
    let mut forbidden = false;
    let mut tries = Tries::new(shared, &way, clients);
    let outcome = loop {
        let Some((session, ch)) = tries.next().await else {
            break tries.outcome();
        };
        let client = session.name();
        let allowed = || {
            shared.still_granted(
                visit.visitor,
                visit.secret,
                visit.request,
                visit.peer.ip(),
                grant,
                client,
            ) && session.admitted(shared)
        };
        if !allowed() {
            forbidden = true;
            break Outcome::Gone(None);
        }
        let wait = shared.settings().heartbeat_interval * 3;
        let opened = match open_udp(ch, &connect, wait).await {
            Ok(link) => link,
            Err(failed) => {
                let error = match failed {
                    Opening::Refused(status) => ForwardError::Status(status),
                    Opening::Silent => crate::forward::no_reply(),
                    Opening::Lost => ForwardError::Protocol,
                };
                match tries.opened(&session, Opened::Refused(error)) {
                    Some(outcome) => break outcome,
                    None => continue,
                }
            }
        };
        way.accepted(shared, client);
        if channel
            .writer
            .send(&mut channel.stream, &[Status::Ok as u8])
            .await
            .is_err()
        {
            break Outcome::Gone(None);
        }
        let meter = Meter {
            limit: grant.service.as_ref().map(|name| shared.limit(name)),
            stats: grant
                .service
                .as_ref()
                .map(|name| shared.metrics.service(name)),
        };
        relay(shared, channel, opened, &session, &visit, meter, &allowed).await;
        break Outcome::Done {
            client: client.to_string(),
            result: Ok(()),
        };
    };
    (outcome, forbidden)
}

async fn relay(
    shared: &Arc<Shared>,
    channel: &mut Channel,
    link: Link,
    session: &Via,
    visit: &Visit<'_>,
    meter: Meter,
    allowed: &impl Fn() -> bool,
) {
    let Link {
        reader: mut creader,
        rd: mut crd,
        writer: mut cwriter,
        wr: mut cwr,
    } = link;
    let Channel {
        stream,
        writer: vwriter,
        reader: vreader,
    } = channel;
    let (mut vrd, mut vwr) = stream.split();
    let label = visit.label;
    let peers = Mutex::new(Peers {
        table: PeerTable::default(),
        gauge: PeerGauge::new(shared.clone()),
        last: Instant::now(),
    });
    let stall = shared.settings().heartbeat_interval * 3;
    let up = async {
        loop {
            let rec = match vreader.recv(&mut vrd).await {
                Ok(Some(rec)) => rec,
                _ => return,
            };
            let Ok((id, payload)) = decode_datagram(rec) else {
                return;
            };
            let admitted = {
                let mut p = peers.lock().expect("peers lock");
                let before = p.table.len();
                let admitted = p
                    .table
                    .id_for(id, || shared.settings().udp_max_peers)
                    .is_some();
                let now = p.table.len();
                if now > before {
                    meter.new_peer();
                }
                p.gauge.set(now);
                if admitted {
                    p.last = Instant::now();
                }
                admitted
            };
            if !admitted {
                static FULL: Throttle = Throttle::new();
                if let Some(more) = FULL.allow() {
                    warn!(
                        "{label}: udp.max_peers {} reached on a UDP visit, dropping a datagram; raise udp.max_peers in server.yaml{more}",
                        shared.settings().udp_max_peers
                    );
                }
                continue;
            }
            if !meter.take_in(payload.len()) {
                continue;
            }
            meter.count_in(payload.len());
            match tokio::time::timeout(stall, cwriter.send(&mut cwr, rec)).await {
                Ok(Ok(())) => {}
                _ => return,
            }
        }
    };
    let down = async {
        loop {
            let rec = match creader.recv(&mut crd).await {
                Ok(Some(rec)) => rec,
                _ => return,
            };
            let Ok((id, payload)) = decode_datagram(rec) else {
                return;
            };
            if !meter.take_out(payload.len()) {
                continue;
            }
            let known = {
                let mut p = peers.lock().expect("peers lock");
                let known = p.table.refresh(&id);
                if known {
                    p.last = Instant::now();
                }
                known
            };
            if !known {
                continue;
            }
            meter.count_out(payload.len());
            match tokio::time::timeout(stall, vwriter.send(&mut vwr, rec)).await {
                Ok(Ok(())) => {}
                _ => return,
            }
        }
    };
    tokio::pin!(up, down);
    let mut sweep = tokio::time::interval(SWEEP);
    let mut reloads = shared.reloads();
    let mut stop = shared.shutdown_signal();
    loop {
        tokio::select! {
            _ = &mut up => break,
            _ = &mut down => break,
            _ = sweep.tick() => {
                let timeout = shared.settings().udp_session_timeout;
                let mut p = peers.lock().expect("peers lock");
                p.table.expire(timeout);
                let now = p.table.len();
                p.gauge.set(now);
                if p.last.elapsed() >= timeout {
                    debug!(peer = %visit.peer, "{label}: UDP visit idle for {}s, closing it", timeout.as_secs());
                    break;
                }
            }
            Ok(()) = reloads.changed() => {
                if !allowed() {
                    info!("{label} may no longer reach {}, closing its UDP visit", visit.grant.target);
                    break;
                }
                if let Some(name) = moved(shared, visit) {
                    info!("{label}: UDP visit to service \"{name}\" closed: its target changed");
                    break;
                }
            }
            _ = session.closed() => break,
            _ = stop.changed() => break,
        }
    }
}

fn moved<'a>(shared: &Shared, visit: &'a Visit<'_>) -> Option<&'a str> {
    let VisitTarget::Service { name, index } = &visit.request.to else {
        return None;
    };
    let now = shared
        .settings()
        .services
        .get(name)
        .map(|svc| svc.target_at(*index));
    (now.as_deref() != Some(visit.grant.target.as_str())).then_some(name.as_str())
}
