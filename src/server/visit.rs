use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tracing::{debug, info, warn};

use super::Shared;
use super::route::{Outcome, Tries, Way};
use super::tcp::log_refusal;
use crate::config::check::{MAX_TARGET_LEN, parse_host_port};
use crate::forward::{ForwardError, Meter, Opened, Opening, relay, reply};
use burrow_config::Protocol;

use crate::policy::throttle::Throttle;
use crate::wire::handshake::{Accepted, MAX_REQUEST_RECORD, read_small};
use crate::wire::protocol::{Connect, Status, VisitRequest, VisitTarget};
use crate::wire::secret::Secret;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Grant {
    pub clients: Vec<String>,
    pub target: String,
    pub service: Option<String>,
    pub only: Option<Vec<String>>,
}

pub(super) struct Refusal {
    pub status: Status,
    pub why: String,
}

impl Shared {
    pub(super) fn grant(
        &self,
        visitor: &str,
        secret: &Secret,
        to: &VisitTarget,
        protocol: Protocol,
        from: IpAddr,
    ) -> Result<Grant, Refusal> {
        let settings = self.settings();
        let forbidden = |why: String| Refusal {
            status: Status::Forbidden,
            why,
        };
        let auth = settings
            .visitors
            .get(visitor)
            .filter(|v| v.secrets.contains(secret))
            .ok_or_else(|| forbidden(format!("visitor \"{visitor}\" is no longer allowed")))?;
        match to {
            VisitTarget::Service { name, index } => {
                let Some(svc) = settings.services.get(name) else {
                    return Err(forbidden(format!(
                        "visitor \"{visitor}\" asked for service \"{name}\", which does not exist"
                    )));
                };
                if !svc.visitors.iter().any(|v| v == visitor) {
                    return Err(forbidden(format!(
                        "visitor \"{visitor}\" may not use service \"{name}\"; to allow it, add it to \"visitors\" of service \"{name}\""
                    )));
                }
                if svc.protocol != protocol {
                    return Err(Refusal {
                        status: Status::Invalid,
                        why: format!(
                            "visitor \"{visitor}\" asked for service \"{name}\" over {}, but it is a {} service; set \"protocol: {}\" on its forward in visitor.yaml",
                            protocol.wire(),
                            svc.protocol.wire(),
                            svc.protocol.wire()
                        ),
                    });
                }
                if !svc.sources.permits(from) {
                    return Err(forbidden(format!(
                        "visitor \"{visitor}\" connects from {from}, which allow_from or deny_from of service \"{name}\" does not permit"
                    )));
                }
                if *index >= svc.ports.max(1) {
                    return Err(Refusal {
                        status: Status::Invalid,
                        why: format!(
                            "visitor \"{visitor}\" asked for port {} of service \"{name}\", which has {} {}",
                            index + 1,
                            svc.ports.max(1),
                            crate::config::diagnostics::plural(svc.ports.max(1).into(), "port")
                        ),
                    });
                }
                Ok(Grant {
                    clients: svc.clients.clone(),
                    target: svc.target_at(*index),
                    service: Some(name.clone()),
                    only: None,
                })
            }
            VisitTarget::Client { client, target } => {
                let Some(rules) = auth.reach.get(client) else {
                    return Err(forbidden(format!(
                        "visitor \"{visitor}\" may not reach client \"{client}\" directly; to allow it, add \"reach: {{ {client}: any }}\" to visitor \"{visitor}\""
                    )));
                };
                let valid = target.is_empty()
                    || match crate::config::check::unix::path_of(target) {
                        Some(path) => crate::config::check::unix::check_path(path).is_ok(),
                        None => target.len() <= MAX_TARGET_LEN && parse_host_port(target).is_ok(),
                    };
                if !valid {
                    return Err(Refusal {
                        status: Status::Invalid,
                        why: format!("visitor \"{visitor}\" asked for an invalid target"),
                    });
                }
                if protocol == Protocol::Udp
                    && crate::config::check::unix::path_of(target).is_some()
                {
                    return Err(Refusal {
                        status: Status::Invalid,
                        why: format!(
                            "visitor \"{visitor}\" asked for UDP to {target}, but a Unix socket target carries TCP only"
                        ),
                    });
                }
                let outside = match crate::config::check::unix::path_of(target) {
                    Some(path) => !rules.permits_socket(path),
                    None => target
                        .parse::<SocketAddr>()
                        .is_ok_and(|addr| !rules.permits(addr)),
                };
                if outside {
                    return Err(forbidden(format!(
                        "visitor \"{visitor}\" asked for {target} on client \"{client}\", which its reach does not include ({})",
                        rules.rules().unwrap_or_default().join(", ")
                    )));
                }
                Ok(Grant {
                    clients: vec![client.clone()],
                    target: target.clone(),
                    service: None,
                    only: rules.rules(),
                })
            }
        }
    }

    pub(super) fn still_granted(
        &self,
        visitor: &str,
        secret: &Secret,
        request: &VisitRequest,
        from: IpAddr,
        grant: &Grant,
        client: &str,
    ) -> bool {
        let checked_here = crate::config::check::unix::path_of(&grant.target).is_some()
            || grant.target.parse::<SocketAddr>().is_ok();
        self.grant(visitor, secret, &request.to, request.protocol, from)
            .is_ok_and(|g| {
                g.service == grant.service
                    && (g.only.is_none() || g.only == grant.only || checked_here)
                    && g.clients.iter().any(|c| c == client)
            })
    }
}

async fn answer_grants(
    shared: &Shared,
    visitor: &str,
    secret: &Secret,
    channel: crate::wire::transport::Channel,
    with_protocols: bool,
) {
    let settings = shared.settings();
    let allowed = settings
        .visitors
        .get(visitor)
        .is_some_and(|v| v.secrets.contains(secret));
    let grants = allowed
        .then(|| crate::config::grants::for_visitor(&settings, visitor))
        .flatten();
    let Some(mut grants) = grants else {
        reply(channel, Status::Forbidden).await;
        return;
    };
    shared.metrics.visitor(visitor).seen();
    for c in &mut grants.clients {
        if c.reach.is_empty() {
            c.allow = shared.via(&c.name).and_then(|s| s.allow());
        }
    }
    let encoded = if with_protocols {
        grants.encode_v2()
    } else {
        grants.encode()
    };
    let Ok(bytes) = encoded else {
        static LARGE: Throttle = Throttle::new();
        if let Some(more) = LARGE.allow() {
            warn!(
                "visitor \"{visitor}\" asked what it may use, but the answer does not fit in one record ({} services, {} clients){more}",
                grants.services.len(),
                grants.clients.len()
            );
        }
        reply(channel, Status::Invalid).await;
        return;
    };
    debug!("visitor \"{visitor}\" asked what it may use");
    crate::forward::send_and_close(channel, &bytes).await;
}

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
    let request = match request {
        Ok(r)
            if r.check
                || !matches!(&r.to, VisitTarget::Client { target, .. } if target.is_empty()) =>
        {
            r
        }
        _ => {
            reply(channel, Status::Invalid).await;
            return;
        }
    };
    let grant = match shared.grant(&visitor, &secret, &request.to, request.protocol, peer.ip()) {
        Ok(g) => {
            if request.check {
                shared.metrics.visitor(&visitor).seen();
            }
            g
        }
        Err(r) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            static REFUSED: Throttle = Throttle::new();
            if let Some(more) = REFUSED.allow() {
                warn!("{}{more}", r.why);
            }
            reply(channel, r.status).await;
            return;
        }
    };
    let label = format!("visitor \"{visitor}\"");
    let clients = match &grant.service {
        Some(name) => shared
            .settings()
            .services
            .get(name)
            .map(|svc| shared.route(name, svc, &grant.target))
            .unwrap_or_default(),
        None => grant
            .clients
            .first()
            .and_then(|c| shared.via(c))
            .into_iter()
            .collect(),
    };
    if clients.is_empty() {
        if let (Some(name), false) = (&grant.service, request.check) {
            shared.metrics.refuse(name);
        }
        debug!(%peer, "{label}: no client of {} is connected", request.to);
        reply(channel, Status::Offline).await;
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
    let stats = grant
        .service
        .as_ref()
        .map(|name| shared.metrics.service(name));
    if let Some(s) = stats.as_ref().filter(|_| !udp) {
        s.total.fetch_add(1, Ordering::Relaxed);
    }
    let busy = |why: String| {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        if let Some(s) = &stats {
            s.refused.fetch_add(1, Ordering::Relaxed);
        }
        debug!(%peer, "{why}");
    };
    let (_visitor_count, before) = shared.visitor_counts.enter(&visitor);
    let visitor_limit = settings
        .visitors
        .get(&visitor)
        .and_then(|v| v.max_connections);
    if visitor_limit.is_some_and(|m| before >= m) {
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
            several: service.is_some_and(|(_, svc)| svc.clients.len() > 1),
        };
        let (outcome, forbidden) =
            super::visit_udp::serve(&shared, &mut channel, visit, clients).await;
        answer(channel, outcome, &grant, &label, peer, &busy, forbidden).await;
        return;
    }
    let _service_count = match service {
        Some((name, svc)) => {
            let (guard, before) = shared.service_counts.enter(name);
            if svc.max_connections.is_some_and(|m| before >= m) {
                busy(format!("service \"{name}\" is at its connection limit"));
                reply(channel, Status::Busy).await;
                return;
            }
            Some(guard)
        }
        None => None,
    };
    let _tracked = shared.track();
    let first = Connect {
        protocol: Protocol::Tcp,
        service: grant.service.clone().unwrap_or_default(),
        target: grant.target.clone(),
        only: grant.only.clone(),
    }
    .encode();
    let prefix = match (
        service.and_then(|(_, svc)| svc.proxy_protocol),
        channel.stream.local_addr(),
    ) {
        (Some(version), Ok(local)) => crate::wire::proxy_protocol::header(version, peer, local),
        _ => Vec::new(),
    };
    let opening = Opening {
        first: &first,
        prefix: &prefix,
        plaintext: settings.insecure_plaintext,
        reply_timeout: settings.heartbeat_interval * 3,
    };
    let limit = grant.service.as_ref().map(|name| shared.limit(name));
    let meter = Meter {
        limit: limit.as_deref(),
        traffic: stats.as_ref().map(|s| &s.traffic),
    };
    let way = Way {
        service: grant.service.as_deref(),
        target: &grant.target,
        label: &label,
        peer,
        several: service.is_some_and(|(_, svc)| svc.clients.len() > 1),
    };
    let mut forbidden = false;
    let mut tries = Tries::new(&shared, &way, clients);
    let outcome = loop {
        let Some((session, ch)) = tries.next().await else {
            break tries.outcome();
        };
        let opened = 'attempt: {
            let client = session.name();
            let mut reloads = shared.reloads();
            let allowed = || {
                shared.still_granted(&visitor, &secret, &request, peer.ip(), &grant, client)
                    && session.admitted(&shared)
            };
            if !allowed() {
                forbidden = true;
                break 'attempt Opened::Gone(None);
            }
            let forward = relay(&mut channel, ch, &opening, meter, tries.early(), || {
                way.accepted(&shared, client)
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
    answer(channel, outcome, &grant, &label, peer, &busy, forbidden).await;
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
            result: Err(error),
        } => log_refusal(label, &client, &grant.target, &error),
        Outcome::Refused { client, error } => {
            match (&error, grant.only.is_some()) {
                (ForwardError::Status(Status::Denied), true) => {
                    static OUTSIDE: Throttle = Throttle::new();
                    if let Some(more) = OUTSIDE.allow() {
                        warn!(
                            "{label}: client \"{client}\" refused {}: it is outside the reach of this visitor or the allow list of the client{more}",
                            grant.target
                        );
                    }
                }
                _ => log_refusal(label, &client, &grant.target, &error),
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
