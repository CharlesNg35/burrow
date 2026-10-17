use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::Shared;
use super::route::{Outcome, Tries, Way};
use crate::config::model::Protocol;
use crate::config::settings::Service;
use crate::forward::{ForwardError, Meter, Opened, Opening, open_tcp};
use crate::protocol::{Connect, Status};
use crate::proxy_protocol::header;
use crate::transport::tcp::tune;

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
            Err(e) => {
                static FAILED: crate::throttle::Throttle = crate::throttle::Throttle::new();
                if let Some(more) = FAILED.allow() {
                    warn!("service \"{name}\": accept failed: {e}{more}");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let svc = config.borrow().clone();
        if !shared.source_allowed(&name, peer.ip()) {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
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
            let client = &session.name;
            let mut reloads = shared.reloads();
            let still_open = || match shared.still_open(&name, client, peer.ip(), index) {
                _ if !shared.allows(client, &session.secret) => {
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
        ForwardError::Protocol => warn!("{label}: client \"{client}\" sent an invalid reply"),
        ForwardError::Io(e) => debug!("{label}: client \"{client}\" did not answer: {e}"),
    }
}

pub(super) fn log_status(label: &str, client: &str, target: &str, st: Status) {
    static STATUS: crate::throttle::Throttle = crate::throttle::Throttle::new();
    if st == Status::Ok {
        return;
    }
    let Some(more) = STATUS.allow() else {
        return;
    };
    match st {
        Status::Denied => warn!(
            "{label}: client \"{client}\" refused {target} because it is not in its allow list (add it to \"allow\" in the client config){more}"
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
        Status::Forbidden | Status::Offline | Status::Busy => {
            warn!("{label}: client \"{client}\" sent an unexpected reply{more}")
        }
        Status::Ok => {}
    }
}
