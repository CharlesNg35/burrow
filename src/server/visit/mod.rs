mod grant;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tracing::{debug, info, warn};

use super::Shared;
use super::route::{Outcome, Tries, Way};
use super::tcp::log_refusal;
use crate::forward::{ForwardError, Meter, Opened, Opening, relay, reply};
use burrow_config::Protocol;

use crate::policy::throttle::Throttle;
use crate::wire::handshake::{Accepted, MAX_REQUEST_RECORD, read_small};
use crate::wire::protocol::{Connect, Status, VisitRequest};
use crate::wire::secret::Secret;
pub(super) use grant::Grant;
use grant::{answer_grants, clients_for, granted, usable};

pub(super) async fn serve(
    shared: Arc<Shared>,
    acc: Accepted,
    peer: SocketAddr,
    deadline: Instant,
    permit: Option<super::listen::Permit>,
) {
    let Accepted {
        preamble,
        mut channel,
        secret,
        ..
    } = acc;
    let visitor = preamble.name;
    if preamble.plaintext {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        debug!(%peer, "visitor \"{visitor}\" asked for insecure_plaintext, closing");
        return;
    }
    let first = tokio::time::timeout_at(
        deadline.into(),
        read_small(&mut channel.stream, &mut channel.reader, MAX_REQUEST_RECORD),
    )
    .await;
    drop(permit);
    let request = match first {
        Ok(Ok(b)) if b[..] == crate::wire::protocol::GRANTS_REQUEST => {
            answer_grants(&shared, &visitor, &secret, channel, false).await;
            return;
        }
        Ok(Ok(b)) if b[..] == crate::wire::protocol::GRANTS_REQUEST_V2 => {
            answer_grants(&shared, &visitor, &secret, channel, true).await;
            return;
        }
        Ok(Ok(b)) if burrow_admin_proto::frame::AdminOpen::is_admin_open(&b) => {
            let opened = super::admin_hook::Opened {
                visitor,
                secret,
                peer,
                via_websocket: channel.stream.is_websocket(),
            };
            super::admin_hook::serve(shared, channel, &b, opened).await;
            return;
        }
        Ok(Ok(b)) => VisitRequest::decode(&b),
        _ => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "visitor \"{visitor}\" sent no valid request");
            return;
        }
    };
    let Some(request) = usable(request) else {
        reply(channel, Status::Invalid).await;
        return;
    };
    let grant = match granted(&shared, &visitor, &secret, &request, peer) {
        Ok(g) => g,
        Err(status) => {
            reply(channel, status).await;
            return;
        }
    };
    let label = format!("visitor \"{visitor}\"");
    let clients = clients_for(&shared, &grant);
    if clients.is_empty() {
        offline(&shared, &grant, &request, peer, &label, channel).await;
        return;
    }
    if request.check {
        reply(channel, Status::Ok).await;
        return;
    }
    let settings = shared.settings();
    let visitor_stats = shared.metrics.visitor(&visitor);
    visitor_stats.total.fetch_add(1, Ordering::Relaxed);
    let _seen = crate::metrics::Seen::new(visitor_stats);
    let udp = request.protocol == Protocol::Udp;
    let stats = counted_service(&shared, &grant, udp);
    let busy = |why: String| {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        if let Some(s) = &stats {
            s.refused.fetch_add(1, Ordering::Relaxed);
        }
        debug!(%peer, "{why}");
    };
    let (_visitor_count, before) = shared.visitor_counts.enter(&visitor);
    if visitor_limit(&settings, &visitor).is_some_and(|m| before >= m) {
        busy(format!("{label} is at its connection limit"));
        reply(channel, Status::Busy).await;
        return;
    }
    let service = grant
        .service
        .as_ref()
        .and_then(|name| settings.services.get(name).map(|svc| (name, svc)));
    if udp {
        let _tracked = shared.track();
        let visit = super::visit_udp::Visit {
            visitor: &visitor,
            secret: &secret,
            request: &request,
            peer,
            grant: &grant,
            label: &label,
            several: service.is_some_and(|(_, svc)| svc.several()),
        };
        let (outcome, forbidden) =
            super::visit_udp::serve(&shared, &mut channel, visit, clients).await;
        answer(channel, outcome, &grant, &label, peer, &busy, forbidden).await;
        return;
    }
    let _service_count = match service_slot(&shared, service, &busy) {
        Ok(guard) => guard,
        Err(_held) => {
            reply(channel, Status::Busy).await;
            return;
        }
    };
    let _tracked = shared.track();
    let prefix = proxy_prefix(
        service.and_then(|(_, svc)| svc.proxy_protocol),
        &channel,
        peer,
    );
    let limit = grant.service.as_ref().map(|name| shared.limit(name));
    let meter = Meter {
        limit: limit.as_deref(),
        traffic: stats.as_ref().map(|s| &s.traffic),
    };
    let way = Way {
        service: grant.service.as_deref(),
        counted: grant.service.as_deref(),
        label: &label,
        peer,
        source: None,
        several: service.is_some_and(|(_, svc)| svc.several()),
    };
    let attempt = Attempt {
        visitor: &visitor,
        secret: &secret,
        request: &request,
        peer,
        grant: &grant,
        label: &label,
        prefix: &prefix,
        settings: &settings,
        meter,
        way: &way,
    };
    let (outcome, forbidden) = forward_tcp(&shared, &mut channel, attempt, clients).await;
    answer(channel, outcome, &grant, &label, peer, &busy, forbidden).await;
}

async fn offline(
    shared: &Shared,
    grant: &Grant,
    request: &VisitRequest,
    peer: SocketAddr,
    label: &str,
    channel: crate::wire::transport::Channel,
) {
    if let (Some(name), false) = (&grant.service, request.check) {
        shared.metrics.refuse(name);
    }
    debug!(%peer, "{label}: no client of {} is connected", request.to);
    reply(channel, Status::Offline).await;
}

fn counted_service(
    shared: &Shared,
    grant: &Grant,
    udp: bool,
) -> Option<Arc<crate::metrics::ServiceStats>> {
    let stats = grant
        .service
        .as_ref()
        .map(|name| shared.metrics.service(name));
    if let Some(s) = stats.as_ref().filter(|_| !udp) {
        s.total.fetch_add(1, Ordering::Relaxed);
    }
    stats
}

fn visitor_limit(
    settings: &crate::config::settings::ServerSettings,
    visitor: &str,
) -> Option<usize> {
    settings
        .visitors
        .get(visitor)
        .and_then(|v| v.max_connections)
}

fn proxy_prefix(
    version: Option<burrow_config::ProxyVersion>,
    channel: &crate::wire::transport::Channel,
    peer: SocketAddr,
) -> Vec<u8> {
    match (version, channel.stream.local_addr()) {
        (Some(version), Ok(local)) => crate::wire::proxy_protocol::header(version, peer, local),
        _ => Vec::new(),
    }
}

fn service_slot(
    shared: &Shared,
    service: Option<(&String, &crate::config::settings::Service)>,
    busy: &impl Fn(String),
) -> Result<Option<super::CountGuard>, super::CountGuard> {
    match service {
        Some((name, svc)) => {
            let (guard, before) = shared.service_counts.enter(name);
            if svc.max_connections.is_some_and(|m| before >= m) {
                busy(format!("service \"{name}\" is at its connection limit"));
                return Err(guard);
            }
            Ok(Some(guard))
        }
        None => Ok(None),
    }
}

fn opening_for<'a>(
    first: &'a [u8],
    prefix: &'a [u8],
    settings: &crate::config::settings::ServerSettings,
) -> Opening<'a> {
    Opening {
        first,
        prefix,
        plaintext: settings.insecure_plaintext,
        reply_timeout: crate::wire::protocol::peer_timeout(settings.heartbeat_interval),
    }
}

fn connect_frame(grant: &Grant, target: &str) -> Vec<u8> {
    Connect {
        protocol: Protocol::Tcp,
        service: grant.service.clone().unwrap_or_default(),
        target: target.to_string(),
        only: grant.only.clone(),
    }
    .encode()
}

struct Attempt<'a> {
    visitor: &'a str,
    secret: &'a Secret,
    request: &'a VisitRequest,
    peer: SocketAddr,
    grant: &'a Grant,
    label: &'a str,
    prefix: &'a [u8],
    settings: &'a crate::config::settings::ServerSettings,
    meter: Meter<'a>,
    way: &'a Way<'a>,
}

async fn forward_tcp(
    shared: &Arc<Shared>,
    channel: &mut crate::wire::transport::Channel,
    a: Attempt<'_>,
    clients: super::pick::Picked,
) -> (Outcome, bool) {
    let Attempt {
        visitor,
        secret,
        request,
        peer,
        grant,
        label,
        prefix,
        settings,
        meter,
        way,
    } = a;
    let mut forbidden = false;
    let mut tries = Tries::new(shared, way, clients);
    let outcome = loop {
        let Some((session, ch)) = tries.next().await else {
            break tries.outcome();
        };
        let opened = 'attempt: {
            let client = session.name();
            let mut reloads = shared.reloads();
            let allowed = || {
                shared.still_granted(visitor, secret, request, peer.ip(), grant, client)
                    && session.admitted(shared)
            };
            if !allowed() {
                forbidden = true;
                break 'attempt Opened::Gone(None);
            }
            let first = connect_frame(grant, &session.target);
            let opening = opening_for(&first, prefix, settings);
            let forward = relay(&mut *channel, ch, &opening, meter, tries.early(), || {
                way.accepted(shared, &session)
            });
            tokio::pin!(forward);
            loop {
                tokio::select! {
                    biased;
                    Ok(()) = reloads.changed() => {
                        if !allowed() {
                            info!("{label} may no longer reach {}, closing its connection", grant.target);
                            break 'attempt Opened::Gone(None);
                        }
                    }
                    r = &mut forward => break 'attempt r,
                }
            }
        };
        if let Some(outcome) = tries.opened(&session, opened) {
            break outcome;
        }
    };
    (outcome, forbidden)
}

async fn answer(
    channel: crate::wire::transport::Channel,
    outcome: Outcome,
    grant: &Grant,
    label: &str,
    peer: SocketAddr,
    busy: &impl Fn(String),
    forbidden: bool,
) {
    match outcome {
        Outcome::Done { result: Ok(()), .. } => debug!(%peer, "{label}: connection closed"),
        Outcome::Done {
            result: Err(ForwardError::Io(e)),
            ..
        } => debug!(%peer, "{label}: connection ended: {e}"),
        Outcome::Done {
            client,
            target,
            result: Err(error),
        } => log_refusal(label, &client, &target, &error),
        Outcome::Refused {
            client,
            target,
            error,
        } => {
            match (&error, grant.only.is_some()) {
                (ForwardError::Status(Status::Denied), true) => {
                    static OUTSIDE: Throttle = Throttle::new();
                    if let Some(more) = OUTSIDE.allow() {
                        warn!(
                            "{label}: client \"{client}\" refused {target}: it is outside the reach of this visitor or the allow list of the client{more}"
                        );
                    }
                }
                _ => log_refusal(label, &client, &target, &error),
            }
            let status = match error {
                ForwardError::Status(st) => st,
                ForwardError::Protocol | ForwardError::Io(_) => Status::Offline,
            };
            reply(channel, status).await;
        }
        Outcome::Full(client) => {
            busy(format!("client \"{client}\" is at its connection limit"));
            reply(channel, Status::Busy).await;
        }
        Outcome::Unavailable(status) => reply(channel, status).await,
        Outcome::Gone(_) if forbidden => reply(channel, Status::Forbidden).await,
        Outcome::Gone(Some(e)) => debug!(%peer, "{label}: connection ended: {e}"),
        Outcome::Gone(None) => {}
    }
}
