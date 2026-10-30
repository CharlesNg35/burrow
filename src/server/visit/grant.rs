use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;

use tracing::{debug, warn};

use crate::config::check::{MAX_TARGET_LEN, parse_host_port};
use crate::forward::reply;
use crate::policy::throttle::Throttle;
use crate::server::Shared;
use crate::wire::protocol::{Status, VisitRequest, VisitTarget};
use crate::wire::secret::Secret;
use burrow_config::Protocol;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::server) struct Grant {
    pub clients: Vec<String>,
    pub target: String,
    pub service: Option<String>,
    pub only: Option<Vec<String>>,
}

pub(in crate::server) struct Refusal {
    pub status: Status,
    pub why: String,
}

impl Shared {
    pub(in crate::server) fn grant(
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

    pub(in crate::server) fn still_granted(
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

pub(super) async fn answer_grants(
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

pub(super) fn granted(
    shared: &Shared,
    visitor: &str,
    secret: &Secret,
    request: &VisitRequest,
    peer: SocketAddr,
) -> Result<Grant, Status> {
    match shared.grant(visitor, secret, &request.to, request.protocol, peer.ip()) {
        Ok(g) => {
            if request.check {
                shared.metrics.visitor(visitor).seen();
            }
            Ok(g)
        }
        Err(r) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            static REFUSED: Throttle = Throttle::new();
            if let Some(more) = REFUSED.allow() {
                warn!("{}{more}", r.why);
            }
            Err(r.status)
        }
    }
}

pub(super) fn usable(
    request: Result<VisitRequest, crate::wire::protocol::DecodeError>,
) -> Option<VisitRequest> {
    match request {
        Ok(r)
            if r.check
                || !matches!(&r.to, VisitTarget::Client { target, .. } if target.is_empty()) =>
        {
            Some(r)
        }
        _ => None,
    }
}

pub(super) fn clients_for(shared: &Shared, grant: &Grant) -> crate::server::pick::Picked {
    match &grant.service {
        Some(name) => shared
            .settings()
            .services
            .get(name)
            .map(|svc| shared.route(name, svc, &grant.target))
            .unwrap_or_else(|| crate::server::pick::Picked::from(Vec::new())),
        None => grant
            .clients
            .first()
            .and_then(|c| shared.via(c))
            .into_iter()
            .collect::<Vec<_>>()
            .into(),
    }
}
