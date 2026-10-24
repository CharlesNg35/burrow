use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::Shared;
use super::route::{Outcome, Tries, Way};
use crate::config::settings::Service;
use crate::forward::{ForwardError, Meter, Opened, Opening, open_tcp};
use crate::wire::protocol::{Connect, Status};
use crate::wire::proxy_protocol::header;
use crate::wire::transport::tcp::tune;
use burrow_config::Protocol;

pub(super) async fn tcp_service(
    shared: Arc<Shared>,
    name: String,
    listener: TcpListener,
    config: watch::Receiver<Service>,
    index: u16,
) {
    loop {
        let (visitor, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) if crate::wire::transport::tcp::gone_before_accept(&e) => {
                debug!("service \"{name}\": a connection was gone before it was accepted: {e}");
                continue;
            }
            Err(e) => {
                static FAILED: crate::policy::throttle::Throttle =
                    crate::policy::throttle::Throttle::new();
                if let Some(more) = FAILED.allow() {
                    warn!("service \"{name}\": accept failed: {e}{more}");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let svc = config.borrow().clone();
        if !shared.source_allowed(&name, peer.ip()) {
            shared.metrics.refuse(&name);
            debug!(%peer, "service \"{name}\": source address not allowed, closing");
            continue;
        }
        let shared = shared.clone();
        let name = name.clone();
        tokio::spawn(async move {
            visit(shared, name, svc, visitor, peer, index).await;
        });
    }
}

async fn visit(
    shared: Arc<Shared>,
    name: String,
    svc: Service,
    mut visitor: TcpStream,
    peer: SocketAddr,
    index: u16,
) {
    let settings = shared.settings();
    let target = svc.target_at(index);
    let stats = shared.metrics.service(&name);
    stats.total.fetch_add(1, Ordering::Relaxed);
    let refuse = || stats.refused.fetch_add(1, Ordering::Relaxed);
    let (_guard, n) = shared.service_counts.enter(&name);
    if svc.max_connections.is_some_and(|m| n >= m) {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        refuse();
        debug!(%peer, "service \"{name}\" is at its connection limit");
        return;
    }
    let clients = shared.route(&name, &svc, &target);
    if clients.is_empty() {
        refuse();
        debug!(%peer, "service \"{name}\": no client is connected, dropping visitor");
        return;
    }
    let _tracked = shared.track();
    tune(&visitor, &settings.tcp);
    let label = format!("service \"{name}\"");
    let first = Connect {
        protocol: Protocol::Tcp,
        service: name.clone(),
        target: target.clone(),
        only: None,
    }
    .encode();
    let prefix = match (svc.proxy_protocol, visitor.local_addr()) {
        (Some(version), Ok(local)) => header(version, peer, local),
        _ => Vec::new(),
    };
    let opening = Opening {
        first: &first,
        prefix: &prefix,
        plaintext: settings.insecure_plaintext,
        reply_timeout: settings.heartbeat_interval * 3,
    };
    let limit = shared.limit(&name);
    let meter = Meter {
        limit: Some(&limit),
        traffic: Some(&stats.traffic),
    };
    let way = Way {
        service: Some(&name),
        target: &target,
        label: &label,
        peer,
        several: svc.clients.len() > 1,
    };
    let mut tries = Tries::new(&shared, &way, clients);
    let outcome = loop {
        let Some((session, ch)) = tries.next().await else {
            break tries.outcome();
        };
        let opened = 'attempt: {
            let client = session.name();
            let mut reloads = shared.reloads();
            let public = svc.bind.map(|b| (svc.protocol, b));
            let still_open = || match shared.still_open(&name, client, peer.ip(), index, public) {
                _ if !session.admitted(&shared) => {
                    Err(format!("client \"{client}\" is no longer allowed"))
                }
                r => r,
            };
            if let Err(why) = still_open() {
                debug!(%peer, "service \"{name}\": {why}, closing");
                break 'attempt Opened::Gone(None);
            }
            let forward = open_tcp(ch, &mut visitor, &opening, meter, tries.early(), || {
                way.accepted(&shared, client)
            });
            tokio::pin!(forward);
            loop {
                tokio::select! {
                    biased;
                    Ok(()) = reloads.changed() => {
                        if let Err(why) = still_open() {
                            info!("service \"{name}\": {why}, closing the connection from {peer}");
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
    if matches!(
        &outcome,
        Outcome::Done {
            result: Err(ForwardError::Status(Status::Busy)),
            ..
        } | Outcome::Refused {
            error: ForwardError::Status(Status::Busy),
            ..
        }
    ) {
        refuse();
    }
    if matches!(outcome, Outcome::Done { result: Err(_), .. }) {
        let _ = socket2::SockRef::from(&visitor).set_linger(Some(Duration::ZERO));
    }
    match outcome {
        Outcome::Done { result: Ok(()), .. } => {
            debug!(%peer, "service \"{name}\": connection closed")
        }
        Outcome::Done {
            result: Err(ForwardError::Io(e)),
            ..
        } => debug!(%peer, "service \"{name}\": connection ended: {e}"),
        Outcome::Done {
            client,
            result: Err(error),
        } => log_refusal(&label, &client, &target, &error),
        Outcome::Refused { client, error } => log_refusal(&label, &client, &target, &error),
        Outcome::Full(client) => {
            debug!(%peer, "client \"{client}\" is at its connection limit");
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            refuse();
        }
        Outcome::Unavailable(_) => {}
        Outcome::Gone(Some(e)) => debug!(%peer, "service \"{name}\": connection ended: {e}"),
        Outcome::Gone(None) => {}
    }
}

pub(super) fn log_refusal(label: &str, client: &str, target: &str, error: &ForwardError) {
    match error {
        ForwardError::Status(st) => log_status(label, client, target, *st),
        ForwardError::Protocol => {
            static INVALID: crate::policy::throttle::Throttle =
                crate::policy::throttle::Throttle::new();
            if let Some(more) = INVALID.allow() {
                warn!("{label}: client \"{client}\" sent an invalid reply{more}");
            }
        }
        ForwardError::Io(e) => debug!("{label}: client \"{client}\" did not answer: {e}"),
    }
}

pub(super) fn log_status(label: &str, client: &str, target: &str, st: Status) {
    static STATUS: crate::policy::throttle::Throttle = crate::policy::throttle::Throttle::new();
    if st == Status::Ok {
        return;
    }
    let Some(more) = STATUS.allow() else {
        return;
    };
    match st {
        Status::Denied if client == burrow_config::SELF => warn!(
            "{label}: this server (\"{client}\") refused {target}: self.allow does not permit it, or it is burrow's own listener or a link-local address that self.allow does not name; the warning before this one names the reason{more}"
        ),
        Status::Denied => warn!(
            "{label}: client \"{client}\" refused {target}: it is not in its allow list (add it to \"allow\" in the client config) or failed a check there, which the client's log names{more}"
        ),
        Status::Unreachable => warn!(
            "{label}: client \"{client}\" could not connect to {target} (is it running and reachable from the client?){more}"
        ),
        Status::Unresolved => {
            warn!("{label}: client \"{client}\" could not resolve {target}{more}")
        }
        Status::Invalid => {
            warn!("{label}: client \"{client}\" rejected the target {target} as invalid{more}")
        }
        Status::Busy => warn!("{label}: {}{more}", busy(client)),
        Status::Forbidden | Status::Offline => {
            warn!("{label}: client \"{client}\" sent an unexpected reply{more}")
        }
        Status::Ok => {}
    }
}

fn busy(client: &str) -> String {
    if client == burrow_config::SELF {
        "this server has too many connections open to its own targets to take another; the open file limit sets how many".into()
    } else {
        format!(
            "client \"{client}\" answered busy: it has reached its max_connections; raise max_connections in its client.yaml"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::busy;

    #[test]
    fn a_busy_reply_points_to_the_limit_that_was_hit() {
        assert_eq!(
            busy("home"),
            "client \"home\" answered busy: it has reached its max_connections; raise max_connections in its client.yaml"
        );
        let own = busy(burrow_config::SELF);
        assert!(!own.contains("client.yaml"), "{own}");
        assert!(own.contains("own targets"), "{own}");
    }
}
