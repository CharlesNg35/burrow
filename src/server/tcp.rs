use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::Shared;
use super::route::{Outcome, Tries, Way};
use super::share::{FairShare, Pool};
use crate::config::settings::Service;
use crate::forward::{ForwardError, Incoming, Meter, Opened, Opening, Split, open_tcp};
use crate::wire::protocol::{Connect, Status};
use crate::wire::proxy_protocol::header;
use crate::wire::transport::tcp::tune;
use burrow_config::Protocol;

pub(super) async fn tcp_service(
    shared: Arc<Shared>,
    name: String,
    listener: TcpListener,
    config: watch::Receiver<Arc<Service>>,
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
        let verdict = shared.admission(&name, peer.ip());
        if !verdict.admitted() {
            debug!(%peer, "service \"{name}\": {}, closing", refused_why(verdict));
            continue;
        }
        let shared = shared.clone();
        let name = name.clone();
        tokio::spawn(async move {
            let Ok(admitted) = admit(&shared, &name, &svc, (peer, None), index) else {
                return;
            };
            tune(&visitor, &shared.settings().tcp);
            let entry = Entry {
                peer,
                local: visitor.local_addr().ok(),
                index,
                door: Door::Bind,
            };
            let shape = Shape::of(&svc, &entry);
            drop(svc);
            visit(shared, name, shape, Incoming::Tcp(visitor), entry, admitted).await;
        });
    }
}

pub(super) enum Door {
    Bind,
    #[cfg(feature = "web")]
    Web(String),
    #[cfg(feature = "web")]
    Pool(burrow_web::Ending),
}

impl Door {
    fn opener(&self, peer: SocketAddr) -> Option<IpAddr> {
        match self {
            Door::Bind => Some(peer.ip()),
            #[cfg(feature = "web")]
            Door::Web(_) => Some(peer.ip()),
            #[cfg(feature = "web")]
            Door::Pool(_) => None,
        }
    }

    fn accepted(&self, hop: &super::pick::Hop, several: bool) {
        #[cfg(feature = "web")]
        if let Door::Pool(ending) = self
            && several
        {
            ending.answered_by(super::web::way_id(hop.name(), &hop.target));
        }
        let _ = (hop, several);
    }
}

pub(super) struct Entry {
    pub peer: SocketAddr,
    pub local: Option<SocketAddr>,
    pub index: u16,
    pub door: Door,
}

pub(super) struct Shape {
    prefix: Vec<u8>,
    several: bool,
    public: Option<(Protocol, SocketAddr)>,
}

impl Shape {
    pub(super) fn of(svc: &Service, entry: &Entry) -> Shape {
        Shape {
            prefix: proxy_prefix(svc, entry.peer, entry.local),
            several: svc.several(),
            public: public_of(svc, &entry.door),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    Full,
    NoClient,
}

pub(super) struct Admitted {
    stats: Arc<crate::metrics::ServiceStats>,
    clients: super::pick::Picked,
    source: Option<IpAddr>,
    _guard: super::CountGuard,
    _share: Option<FairShare>,
    _tracked: Option<super::ActiveConn>,
}

pub(super) fn admit(
    shared: &Arc<Shared>,
    name: &str,
    svc: &Service,
    (peer, source): (SocketAddr, Option<IpAddr>),
    index: u16,
) -> Result<Admitted, Refusal> {
    let mut admitted = enter(shared, name, svc, (peer, source), || {
        shared.route(name, svc, index, Some(peer.ip()))
    })?;
    admitted._tracked = Some(shared.track());
    Ok(admitted)
}

#[cfg(feature = "web")]
pub(super) fn admit_pooled(
    shared: &Arc<Shared>,
    name: &str,
    svc: &Service,
    peer: SocketAddr,
    clients: super::pick::Picked,
) -> Result<Admitted, Refusal> {
    enter(shared, name, svc, (peer, None), || clients)
}

fn enter(
    shared: &Arc<Shared>,
    name: &str,
    svc: &Service,
    (peer, source): (SocketAddr, Option<IpAddr>),
    pick: impl FnOnce() -> super::pick::Picked,
) -> Result<Admitted, Refusal> {
    let stats = shared.metrics.service(name);
    stats.total.fetch_add(1, Ordering::Relaxed);
    let (guard, n) = shared.service_counts.enter(name);
    let full = |why: &str| {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        stats.refused.fetch_add(1, Ordering::Relaxed);
        debug!(%peer, "service \"{name}\" {why}");
        Refusal::Full
    };
    if svc.max_connections.is_some_and(|m| n >= m) {
        return Err(full("is at its connection limit"));
    }
    let share = match svc.max_connections {
        Some(m) => shared
            .visitor_shares
            .take(Pool::Service(name.to_string()), source, m, n)
            .map_err(|()| full("is over half full and this address holds its fair part"))?,
        None => None,
    };
    let clients = pick();
    if clients.is_empty() {
        stats.refused.fetch_add(1, Ordering::Relaxed);
        debug!(%peer, "service \"{name}\": no client is connected, dropping visitor");
        return Err(Refusal::NoClient);
    }
    Ok(Admitted {
        stats,
        clients,
        source,
        _guard: guard,
        _share: share,
        _tracked: None,
    })
}

pub(super) async fn visit(
    shared: Arc<Shared>,
    name: String,
    shape: Shape,
    mut visitor: Incoming,
    entry: Entry,
    admitted: Admitted,
) {
    let settings = shared.settings();
    let Entry {
        peer, index, door, ..
    } = entry;
    let stats = admitted.stats.clone();
    let refuse = || {
        stats.refused.fetch_add(1, Ordering::Relaxed);
    };
    let label = format!("service \"{name}\"");
    let limit = shared.limit(&name);
    let meter = Meter {
        limit: Some(&limit),
        traffic: Some(&stats.traffic),
    };
    let counted = door.opener(peer).map(|_| name.as_str());
    let way = Way {
        service: Some(&name),
        counted,
        label: &label,
        peer,
        source: admitted.source,
        several: shape.several,
    };
    let mut tries = Tries::new(&shared, &way, admitted.clients);
    let outcome = loop {
        let Some((session, ch)) = tries.next().await else {
            break tries.outcome();
        };
        let opened = 'attempt: {
            let client = session.name();
            let mut reloads = shared.reloads();
            let still_open = || {
                if !session.admitted(&shared) {
                    return Err(format!("client \"{client}\" is no longer allowed"));
                }
                shared.still_open(&name, client, door.opener(peer), index, shape.public)?;
                match &door {
                    Door::Bind => Ok(()),
                    #[cfg(feature = "web")]
                    Door::Web(host) => shared.still_on_web(&name, host),
                    #[cfg(feature = "web")]
                    Door::Pool(_) => Ok(()),
                }
            };
            if let Err(why) = still_open() {
                debug!(%peer, "service \"{name}\": {why}, closing");
                break 'attempt Opened::Gone(None);
            }
            let first = connect_frame(&name, &session.target);
            let opening = Opening {
                first: &first,
                prefix: &shape.prefix,
                plaintext: settings.insecure_plaintext,
                reply_timeout: crate::wire::protocol::peer_timeout(settings.heartbeat_interval),
            };
            let forward = open_tcp(ch, &mut visitor, &opening, meter, tries.early(), || {
                way.accepted(&shared, &session);
                door.accepted(&session, shape.several);
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
    match &outcome {
        Outcome::Done { result: Ok(()), .. } => visitor.finished_cleanly(),
        Outcome::Done { result: Err(_), .. } => visitor.reset_on_close(true),
        _ => {}
    }
    #[cfg(feature = "web")]
    visitor.refused(super::web::refusal(&outcome));
    log_outcome(&shared, &label, peer, outcome, refuse);
}

fn connect_frame(name: &str, target: &str) -> Vec<u8> {
    Connect {
        protocol: Protocol::Tcp,
        service: name.to_string(),
        target: target.to_string(),
        only: None,
    }
    .encode()
}

fn proxy_prefix(svc: &Service, peer: SocketAddr, local: Option<SocketAddr>) -> Vec<u8> {
    match (svc.proxy_protocol, local) {
        (Some(version), Some(local)) => header(version, peer, local),
        _ => Vec::new(),
    }
}

fn public_of(svc: &Service, door: &Door) -> Option<(Protocol, SocketAddr)> {
    match door {
        Door::Bind => svc.bind.map(|b| (svc.protocol, b)),
        #[cfg(feature = "web")]
        Door::Web(_) | Door::Pool(_) => None,
    }
}

fn log_outcome(
    shared: &Shared,
    label: &str,
    peer: SocketAddr,
    outcome: Outcome,
    refuse: impl Fn(),
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
        } => log_refusal(label, &client, &target, &error),
        Outcome::Full(client) => {
            debug!(%peer, "client \"{client}\" is at its connection limit");
            shared.rejected.fetch_add(1, Ordering::Relaxed);
            refuse();
        }
        Outcome::Unavailable(_) => {}
        Outcome::Gone(Some(e)) => debug!(%peer, "{label}: connection ended: {e}"),
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
        Status::Offline => warn!(
            "{label}: client \"{client}\" had no free local port to connect to {target}: many recent connections from it are still closing (TIME_WAIT){more}"
        ),
        Status::Forbidden => {
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

pub(super) fn refused_why(verdict: crate::policy::geo::Verdict) -> &'static str {
    match verdict {
        crate::policy::geo::Verdict::Region => "the visitor's country is not allowed",
        _ => "source address not allowed",
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
