use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tracing::{debug, info, warn};

use super::Shared;
use super::route::{Outcome, Tries, Way};
use super::tcp::log_refusal;
use crate::config::check::{MAX_TARGET_LEN, parse_host_port};
use crate::config::model::Protocol;
use crate::forward::{ForwardError, Meter, Opened, Opening, relay, reply};
use tokio::sync::OwnedSemaphorePermit;

use crate::handshake::{Accepted, MAX_REQUEST_RECORD, read_small};
use crate::protocol::{Connect, Status, VisitRequest, VisitTarget};
use crate::secret::Secret;
use crate::throttle::Throttle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Grant {
    pub clients: Vec<String>,
    pub target: String,
    pub service: Option<String>,
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
                if svc.protocol != Protocol::Tcp || !svc.visitors.iter().any(|v| v == visitor) {
                    return Err(forbidden(format!(
                        "visitor \"{visitor}\" may not use service \"{name}\"; to allow it, add it to \"visitors\" of service \"{name}\""
                    )));
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
                            crate::diagnostics::plural(svc.ports.max(1).into(), "port")
                        ),
                    });
                }
                Ok(Grant {
                    clients: svc.clients.clone(),
                    target: svc.target_at(*index),
                    service: Some(name.clone()),
                })
            }
            VisitTarget::Client { client, target } => {
                if !auth.reach.contains(client) {
                    return Err(forbidden(format!(
                        "visitor \"{visitor}\" may not reach client \"{client}\" directly; to allow it, add \"reach: {{ {client}: any }}\" to visitor \"{visitor}\""
                    )));
                }
                let valid = match crate::config::check::unix::path_of(target) {
                    Some(path) => crate::config::check::unix::check_path(path).is_ok(),
                    None => target.len() <= MAX_TARGET_LEN && parse_host_port(target).is_ok(),
                };
                if !valid {
                    return Err(Refusal {
                        status: Status::Invalid,
                        why: format!("visitor \"{visitor}\" asked for an invalid target"),
                    });
                }
                Ok(Grant {
                    clients: vec![client.clone()],
                    target: target.clone(),
                    service: None,
                })
            }
        }
    }

    fn still_granted(
        &self,
        visitor: &str,
        secret: &Secret,
        to: &VisitTarget,
        from: IpAddr,
        grant: &Grant,
        client: &str,
    ) -> bool {
        self.grant(visitor, secret, to, from).is_ok_and(|g| {
            g.target == grant.target
                && g.service == grant.service
                && g.clients.iter().any(|c| c == client)
        })
    }
}

async fn answer_grants(
    shared: &Shared,
    visitor: &str,
    secret: &Secret,
    from: IpAddr,
    mut channel: crate::handshake::Channel,
) {
    let settings = shared.settings();
    let allowed = settings
        .visitors
        .get(visitor)
        .is_some_and(|v| v.secrets.contains(secret));
    let grants = allowed
        .then(|| crate::config::grants::for_visitor(&settings, visitor, Some(from)))
        .flatten();
    let Some(mut grants) = grants else {
        reply(channel, Status::Forbidden).await;
        return;
    };
    for c in &mut grants.clients {
        c.allow = shared.session(&c.name).and_then(|s| s.allow());
    }
    let Ok(bytes) = grants.encode() else {
        warn!(
            "visitor \"{visitor}\" asked what it may use, but the answer does not fit in one record ({} services, {} clients)",
            grants.services.len(),
            grants.clients.len()
        );
        reply(channel, Status::Invalid).await;
        return;
    };
    debug!("visitor \"{visitor}\" asked what it may use");
    let _ = channel.writer.send(&mut channel.stream, &bytes).await;
    let _ = tokio::io::AsyncWriteExt::shutdown(&mut channel.stream).await;
}

pub(super) async fn serve(
    shared: Arc<Shared>,
    acc: Accepted,
    peer: SocketAddr,
    deadline: Instant,
    permit: Option<OwnedSemaphorePermit>,
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
        Ok(Ok(b)) if b[..] == crate::protocol::GRANTS_REQUEST => {
            answer_grants(&shared, &visitor, &secret, peer.ip(), channel).await;
            return;
        }
        Ok(Ok(b)) => VisitRequest::decode(&b),
        _ => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            debug!(%peer, "visitor \"{visitor}\" sent no valid request");
            return;
        }
    };
    let Ok(request) = request else {
        reply(channel, Status::Invalid).await;
        return;
    };
    let grant = match shared.grant(&visitor, &secret, &request.to, peer.ip()) {
        Ok(g) => g,
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
            .and_then(|c| shared.session(c))
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
    shared
        .metrics
        .visitor(&visitor)
        .total
        .fetch_add(1, Ordering::Relaxed);
    let stats = grant
        .service
        .as_ref()
        .map(|name| shared.metrics.service(name));
    if let Some(s) = &stats {
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
    }
    .encode();
    let prefix = match (
        service.and_then(|(_, svc)| svc.proxy_protocol),
        channel.stream.local_addr(),
    ) {
        (Some(version), Ok(local)) => crate::proxy_protocol::header(version, peer, local),
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
            let client = &session.name;
            let mut reloads = shared.reloads();
            let allowed = || {
                shared.still_granted(&visitor, &secret, &request.to, peer.ip(), &grant, client)
                    && shared.allows(client, &session.secret)
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
    match outcome {
        Outcome::Done { result: Ok(()), .. } => debug!(%peer, "{label}: connection closed"),
        Outcome::Done {
            result: Err(ForwardError::Io(e)),
            ..
        } => debug!(%peer, "{label}: connection ended: {e}"),
        Outcome::Done {
            client,
            result: Err(error),
        } => log_refusal(&label, &client, &grant.target, &error),
        Outcome::Refused { client, error } => {
            log_refusal(&label, &client, &grant.target, &error);
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
