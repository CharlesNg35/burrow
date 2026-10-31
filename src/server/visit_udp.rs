use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tracing::{debug, info, warn};

use super::Shared;
use super::route::{Outcome, Tries, Way};
use super::udp::ServiceMeter;
use super::via::Via;
use super::visit::Grant;
use crate::forward::{ForwardError, Opened};
use crate::policy::throttle::Throttle;
use crate::udp::front::Meter;
use crate::udp::link::{Link, Opening, open_udp};
use crate::udp::peers::{PeerGauge, PeerTable};
use crate::wire::protocol::{Connect, Status, VisitRequest, VisitTarget, decode_datagram};
use crate::wire::secret::Secret;
use crate::wire::transport::Channel;
use burrow_config::Protocol;

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

pub(super) async fn serve(
    shared: &Arc<Shared>,
    channel: &mut Channel,
    visit: Visit<'_>,
    clients: super::pick::Picked,
) -> (Outcome, bool) {
    let grant = visit.grant;
    let way = Way {
        service: grant.service.as_deref(),
        label: visit.label,
        peer: visit.peer,
        several: visit.several,
    };
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
        let wait = crate::wire::protocol::peer_timeout(shared.settings().heartbeat_interval);
        let connect = Connect {
            protocol: Protocol::Udp,
            service: grant.service.clone().unwrap_or_default(),
            target: session.target.clone(),
            only: grant.only.clone(),
        }
        .encode();
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
        way.accepted(shared, &session);
        if channel
            .writer
            .send(&mut channel.stream, &[Status::Ok as u8])
            .await
            .is_err()
        {
            break Outcome::Gone(None);
        }
        let meter = match &grant.service {
            Some(name) => ServiceMeter::of(shared, name),
            None => ServiceMeter {
                limit: None,
                stats: None,
            },
        };
        relay(shared, channel, opened, &session, &visit, meter, &allowed).await;
        break Outcome::Done {
            client: client.to_string(),
            target: session.target.clone(),
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
    meter: ServiceMeter,
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
    let stall = crate::wire::protocol::peer_timeout(shared.settings().heartbeat_interval);
    let up = async {
        let mut sealed = Vec::new();
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
            match tokio::time::timeout(stall, cwriter.send_with(&mut cwr, rec, &mut sealed)).await {
                Ok(Ok(())) => {}
                _ => return,
            }
        }
    };
    let down = async {
        let mut sealed = Vec::new();
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
            match tokio::time::timeout(stall, vwriter.send_with(&mut vwr, rec, &mut sealed)).await {
                Ok(Ok(())) => {}
                _ => return,
            }
        }
    };
    tokio::pin!(up, down);
    let mut sweep = tokio::time::interval(crate::udp::front::SWEEP);
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
