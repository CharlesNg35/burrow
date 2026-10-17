use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tracing::{debug, info, warn};

use super::Shared;
use super::tcp::log_status;
use crate::config::check::{MAX_TARGET_LEN, parse_host_port};
use crate::config::model::Protocol;
use crate::forward::{ForwardError, Meter, relay, reply};
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
    let picked = match &grant.service {
        Some(name) => shared
            .settings()
            .services
            .get(name)
            .and_then(|svc| shared.pick(name, svc)),
        None => grant.clients.first().and_then(|c| shared.session(c)),
    };
    let Some(session) = picked else {
        if let (Some(name), false) = (&grant.service, request.check) {
            shared.metrics.refuse(name);
        }
        debug!(%peer, "{label}: no client of {} is connected", request.to);
        reply(channel, Status::Offline).await;
        return;
    };
    let client = session.name.clone();
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
    let _service_count = match &grant.service {
        Some(name) => {
            let (guard, before) = shared.service_counts.enter(name);
            let limit = settings.services.get(name).and_then(|s| s.max_connections);
            if limit.is_some_and(|m| before >= m) {
                busy(format!("service \"{name}\" is at its connection limit"));
                reply(channel, Status::Busy).await;
                return;
            }
            Some(guard)
        }
        None => None,
    };
    let client_limit = settings
        .clients
        .get(&client)
        .and_then(|c| c.max_connections)
        .unwrap_or(settings.max_connections_per_client);
    let (_active, before) = session.connection_guard();
    if before >= client_limit {
        busy(format!("client \"{client}\" is at its connection limit"));
        reply(channel, Status::Busy).await;
        return;
    }
    let _tracked = shared.track();
    let ch = match super::take_channel(&shared, &session, &label, peer).await {
        Ok(ch) => ch,
        Err(status) => {
            reply(channel, status).await;
            return;
        }
    };
    let mut reloads = shared.reloads();
    let allowed = || {
        shared.still_granted(&visitor, &secret, &request.to, peer.ip(), &grant, &client)
            && shared.allows(&client, &session.secret)
    };
    if !allowed() {
        reply(channel, Status::Forbidden).await;
        return;
    }
    let connect = Connect {
        protocol: Protocol::Tcp,
        service: grant.service.clone().unwrap_or_default(),
        target: grant.target.clone(),
    };
    let version = grant
        .service
        .as_ref()
        .and_then(|name| settings.services.get(name))
        .and_then(|s| s.proxy_protocol);
    let prefix = match (version, channel.stream.local_addr()) {
        (Some(version), Ok(local)) => crate::proxy_protocol::header(version, peer, local),
        _ => Vec::new(),
    };
    let limit = grant
        .service
        .as_ref()
        .and_then(|name| shared.limit(name, settings.services.get(name).and_then(|s| s.bandwidth)));
    let forward = relay(
        channel,
        ch,
        &connect,
        &prefix,
        settings.insecure_plaintext,
        Meter {
            limit: limit.as_deref(),
            traffic: stats.as_ref().map(|s| &s.traffic),
        },
    );
    tokio::pin!(forward);
    let result = loop {
        tokio::select! {
            biased;
            Ok(()) = reloads.changed() => {
                if !allowed() {
                    info!("{label} may no longer reach {}, closing its connection", grant.target);
                    return;
                }
            }
            r = &mut forward => break r,
        }
    };
    match result {
        Ok(()) => debug!(%peer, "{label}: connection closed"),
        Err(ForwardError::Status(st)) => log_status(&label, &client, &grant.target, st),
        Err(ForwardError::Protocol) => {
            warn!("{label}: client \"{}\" sent an invalid reply", client)
        }
        Err(ForwardError::Io(e)) => debug!(%peer, "{label}: connection ended: {e}"),
    }
}
