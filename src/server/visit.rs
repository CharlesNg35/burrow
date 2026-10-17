use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tracing::{debug, info, warn};

use super::Shared;
use super::tcp::log_status;
use crate::config::check::{MAX_TARGET_LEN, parse_host_port};
use crate::config::model::Protocol;
use crate::forward::{ForwardError, relay, reply};
use tokio::sync::OwnedSemaphorePermit;

use crate::handshake::{Accepted, MAX_REQUEST_RECORD, read_small};
use crate::protocol::{Connect, Status, VisitRequest, VisitTarget};
use crate::secret::Secret;
use crate::throttle::Throttle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Grant {
    pub client: String,
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
            VisitTarget::Service(name) => {
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
                Ok(Grant {
                    client: svc.client.clone(),
                    target: svc.target.clone(),
                    service: Some(name.clone()),
                })
            }
            VisitTarget::Client { client, target } => {
                if !auth.reach.contains(client) {
                    return Err(forbidden(format!(
                        "visitor \"{visitor}\" may not reach client \"{client}\" directly; to allow it, add \"reach: {{ {client}: any }}\" to visitor \"{visitor}\""
                    )));
                }
                if target.len() > MAX_TARGET_LEN || parse_host_port(target).is_err() {
                    return Err(Refusal {
                        status: Status::Invalid,
                        why: format!("visitor \"{visitor}\" asked for an invalid target"),
                    });
                }
                Ok(Grant {
                    client: client.clone(),
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
        grant: &Grant,
    ) -> bool {
        self.grant(visitor, secret, to).is_ok_and(|g| g == *grant)
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
    let grant = match shared.grant(&visitor, &secret, &request.to) {
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
    let Some(session) = shared.session(&grant.client) else {
        debug!(%peer, "{label}: client \"{}\" is not connected", grant.client);
        reply(channel, Status::Offline).await;
        return;
    };
    if request.check {
        reply(channel, Status::Ok).await;
        return;
    }
    let settings = shared.settings();
    let busy = |why: String| {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
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
        .get(&grant.client)
        .and_then(|c| c.max_connections)
        .unwrap_or(settings.max_connections_per_client);
    let (_active, before) = session.connection_guard();
    if before >= client_limit {
        busy(format!(
            "client \"{}\" is at its connection limit",
            grant.client
        ));
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
        shared.still_granted(&visitor, &secret, &request.to, &grant)
            && shared.allows(&grant.client, &session.secret)
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
    let forward = relay(channel, ch, &connect, settings.insecure_plaintext);
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
        Err(ForwardError::Status(st)) => log_status(&label, &grant.client, &grant.target, st),
        Err(ForwardError::Protocol) => {
            warn!("{label}: client \"{}\" sent an invalid reply", grant.client)
        }
        Err(ForwardError::Io(e)) => debug!(%peer, "{label}: connection ended: {e}"),
    }
}
