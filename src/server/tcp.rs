use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::Shared;
use crate::config::model::Protocol;
use crate::config::settings::Service;
use crate::forward::{ForwardError, Meter, open_tcp};
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
    visitor: TcpStream,
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
    let Some(session) = shared.pick(&name, &svc) else {
        refuse();
        debug!(%peer, "service \"{name}\": no client is connected, dropping visitor");
        return;
    };
    let client = session.name.clone();
    let client_limit = settings
        .clients
        .get(&client)
        .and_then(|c| c.max_connections)
        .unwrap_or(settings.max_connections_per_client);
    let (_active, before) = session.connection_guard();
    if before >= client_limit {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        refuse();
        debug!(%peer, "client \"{client}\" is at its connection limit");
        return;
    }
    let _tracked = shared.track();
    tune(&visitor, &settings.tcp);
    let label = format!("service \"{name}\"");
    let Ok(ch) = super::take_channel(&shared, &session, &label, peer).await else {
        return;
    };
    let connect = Connect {
        protocol: Protocol::Tcp,
        service: name.clone(),
        target: target.clone(),
    };
    let mut reloads = shared.reloads();
    let still_open = || match shared.still_open(&name, &client, peer.ip(), index) {
        _ if !shared.allows(&client, &session.secret) => {
            Err(format!("client \"{client}\" is no longer allowed"))
        }
        r => r,
    };
    if let Err(why) = still_open() {
        debug!(%peer, "service \"{name}\": {why}, closing");
        return;
    }
    let first = connect.encode();
    let prefix = match (svc.proxy_protocol, visitor.local_addr()) {
        (Some(version), Ok(local)) => header(version, peer, local),
        _ => Vec::new(),
    };
    let limit = shared.limit(&name);
    let forward = open_tcp(
        ch,
        visitor,
        &first,
        &prefix,
        settings.insecure_plaintext,
        Meter {
            limit: Some(&limit),
            traffic: Some(&stats.traffic),
        },
        settings.heartbeat_interval * 3,
    );
    tokio::pin!(forward);
    let result = loop {
        tokio::select! {
            biased;
            Ok(()) = reloads.changed() => {
                if let Err(why) = still_open() {
                    info!("service \"{name}\": {why}, closing the connection from {peer}");
                    return;
                }
            }
            r = &mut forward => break r,
        }
    };
    match result {
        Ok(()) => debug!(%peer, "service \"{name}\": connection closed"),
        Err(ForwardError::Status(st)) => log_status(&label, &client, &target, st),
        Err(ForwardError::Protocol) => warn!("service \"{name}\": client sent an invalid reply"),
        Err(ForwardError::Io(e)) => debug!(%peer, "service \"{name}\": connection ended: {e}"),
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
