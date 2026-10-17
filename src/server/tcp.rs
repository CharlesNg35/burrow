use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{debug, warn};

use super::{Shared, TAKE_TIMEOUT, TakeError};
use crate::config::model::Protocol;
use crate::config::settings::Service;
use crate::forward::{ForwardError, server_tcp};
use crate::protocol::{Connect, Status};
use crate::transport::tcp::tune;

pub(super) async fn tcp_service(
    shared: Arc<Shared>,
    name: String,
    listener: TcpListener,
    config: watch::Receiver<Service>,
) {
    let count = Arc::new(AtomicUsize::new(0));
    loop {
        let (visitor, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("service \"{name}\": accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let svc = config.borrow().clone();
        let shared = shared.clone();
        let name = name.clone();
        let count = count.clone();
        tokio::spawn(async move {
            visit(shared, name, svc, visitor, peer, count).await;
        });
    }
}

struct CountGuard(Arc<AtomicUsize>);

impl Drop for CountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn visit(
    shared: Arc<Shared>,
    name: String,
    svc: Service,
    visitor: TcpStream,
    peer: SocketAddr,
    count: Arc<AtomicUsize>,
) {
    let settings = shared.settings();
    let n = count.fetch_add(1, Ordering::AcqRel);
    let _guard = CountGuard(count);
    if svc.max_connections.is_some_and(|m| n >= m) {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        debug!(%peer, "service \"{name}\" is at its connection limit");
        return;
    }
    let Some(session) = shared.session(&svc.client) else {
        debug!(%peer, "service \"{name}\": client \"{}\" is not connected, dropping visitor", svc.client);
        return;
    };
    let client_limit = settings
        .clients
        .get(&svc.client)
        .and_then(|c| c.max_connections)
        .unwrap_or(settings.max_connections_per_client);
    let (_active, before) = session.connection_guard();
    if before >= client_limit {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        debug!(%peer, "client \"{}\" is at its connection limit", svc.client);
        return;
    }
    let _tracked = shared.track();
    tune(&visitor, &settings.tcp);
    let ch = match tokio::time::timeout(TAKE_TIMEOUT, session.take()).await {
        Ok(Ok(ch)) => ch,
        Ok(Err(TakeError::TooManyPending)) => {
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            static WAITING: super::Throttle = super::Throttle::new();
            if let Some(more) = WAITING.allow() {
                warn!(
                    "service \"{name}\": too many visitors waiting for client \"{}\", dropping one{more}",
                    svc.client
                );
            }
            return;
        }
        Ok(Err(TakeError::Closed)) => {
            debug!(%peer, "service \"{name}\": session closed while waiting for a data channel");
            return;
        }
        Err(_) => {
            static SLOW: super::Throttle = super::Throttle::new();
            if let Some(more) = SLOW.allow() {
                warn!(
                    "service \"{name}\": client \"{}\" did not open a data channel within {}s{more}",
                    svc.client,
                    TAKE_TIMEOUT.as_secs()
                );
            }
            return;
        }
    };
    let connect = Connect {
        protocol: Protocol::Tcp,
        service: name.clone(),
        target: svc.target.clone(),
    };
    let mut reloads = shared.reloads();
    if !shared.allows(&svc.client, &session.secret) {
        return;
    }
    let forward = server_tcp(ch, visitor, &connect, settings.insecure_plaintext);
    tokio::pin!(forward);
    let result = loop {
        tokio::select! {
            biased;
            Ok(()) = reloads.changed() => {
                if !shared.allows(&svc.client, &session.secret) {
                    debug!(%peer, "service \"{name}\": client \"{}\" is no longer allowed, closing", svc.client);
                    return;
                }
            }
            r = &mut forward => break r,
        }
    };
    match result {
        Ok(()) => debug!(%peer, "service \"{name}\": connection closed"),
        Err(ForwardError::Status(st)) => log_status(&name, &svc, st),
        Err(ForwardError::Protocol) => warn!("service \"{name}\": client sent an invalid reply"),
        Err(ForwardError::Io(e)) => debug!(%peer, "service \"{name}\": connection ended: {e}"),
    }
}

pub(super) fn log_status(name: &str, svc: &Service, st: Status) {
    static STATUS: super::Throttle = super::Throttle::new();
    if st == Status::Ok {
        return;
    }
    let Some(more) = STATUS.allow() else {
        return;
    };
    let client = &svc.client;
    let target = &svc.target;
    match st {
        Status::Denied => warn!(
            "service \"{name}\": client \"{client}\" refused {target} because it is not in its allow list (add it to \"allow\" in the client config){more}"
        ),
        Status::Unreachable => warn!(
            "service \"{name}\": client \"{client}\" could not connect to {target} (is it running and reachable from the client?){more}"
        ),
        Status::Unresolved => {
            warn!("service \"{name}\": client \"{client}\" could not resolve {target}{more}")
        }
        Status::Invalid => {
            warn!(
                "service \"{name}\": client \"{client}\" rejected the target {target} as invalid{more}"
            )
        }
        Status::Ok => {}
    }
}
