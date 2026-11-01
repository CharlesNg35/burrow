use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use burrow_web::{Ask, BoxIo, Lease, Refused, Visitor, Way};
use tracing::debug;

use super::super::pick::Hop;
use super::super::share::{FairShare, Pool, web_source};
use super::super::tcp::{Door, Entry, Refusal, Shape, admit_pooled, visit};
use super::super::{ActiveConn, CountGuard, Shared};
use crate::config::settings::{ServerSettings, Service};

pub(in crate::server) struct WebLease {
    hop: Option<Hop>,
    _entry: Option<CountGuard>,
    _busy: (CountGuard, CountGuard),
    _shares: (Option<FairShare>, Option<FairShare>),
    _active: ActiveConn,
}

pub(in crate::server) fn way_id(client: &str, target: &str) -> u64 {
    let mut h = DefaultHasher::new();
    (client, target).hash(&mut h);
    h.finish()
}

fn only_client(svc: &Service) -> Option<Arc<str>> {
    match svc.entries.first() {
        Some(e) => Some(e.key.client.clone()),
        None => svc.clients.first().map(|c| Arc::from(c.as_str())),
    }
}

fn client_limit(settings: &ServerSettings, client: &str) -> usize {
    settings
        .clients
        .get(client)
        .and_then(|c| c.max_connections)
        .unwrap_or(settings.max_connections_per_client)
}

pub(in crate::server) fn lease(
    shared: &Arc<Shared>,
    service: &str,
    from: &Visitor,
) -> Result<(Way, Lease), Refused> {
    let settings = shared.settings();
    let svc = settings.services.get(service).ok_or(Refused::Gone)?;
    let (hop, client, id, entry) = if svc.several() {
        let (hops, entry) = shared.lead(service, svc);
        let first = hops.into_iter().next().ok_or(Refused::NoClient)?;
        let id = way_id(first.name(), &first.target);
        let client = first.key.client.clone();
        (Some(first), client, id, entry)
    } else {
        (None, only_client(svc).ok_or(Refused::NoClient)?, 0, None)
    };
    let source = web_source(from.addr);
    let full = |why: &str| {
        shared.rejected.fetch_add(1, Ordering::Relaxed);
        shared
            .metrics
            .service(service)
            .refused
            .fetch_add(1, Ordering::Relaxed);
        debug!(peer = %from.addr, "service \"{service}\" {why}");
        Refused::Full
    };
    let (in_service, busy_service) = shared.busy.services.enter(service);
    let service_share = match svc.max_connections {
        Some(m) => shared
            .visitor_shares
            .take(Pool::Service(service.to_string()), source, m, busy_service)
            .map_err(|()| full("is over half busy and this address holds its fair part"))?,
        None => None,
    };
    let (in_client, busy_client) = shared.busy.clients.enter(&client);
    let limit = client_limit(&settings, &client);
    let client_share = shared
        .visitor_shares
        .take(Pool::Client(client.to_string()), source, limit, busy_client)
        .map_err(|()| {
            full("is over half busy on its client and this address holds its fair part")
        })?;
    let way = Way {
        id,
        client,
        bound: svc.proxy_protocol.map(|_| (from.addr, from.local)),
    };
    let held = WebLease {
        hop,
        _entry: entry,
        _busy: (in_service, in_client),
        _shares: (service_share, client_share),
        _active: shared.track(),
    };
    Ok((way, Lease::new(held)))
}

fn hops_for(shared: &Shared, service: &str, svc: &Service, lease: &WebLease) -> Vec<Hop> {
    let mut hops = shared.ranked(service, svc);
    if let Some(first) = &lease.hop {
        hops.retain(|h| h.key != first.key);
        hops.insert(0, first.clone());
    }
    hops
}

pub(in crate::server) fn open_pooled(
    shared: &Arc<Shared>,
    service: &str,
    from: &Visitor,
    lease: &Lease,
) -> Result<BoxIo, Refused> {
    let settings = shared.settings();
    let svc = settings.services.get(service).ok_or(Refused::Gone)?;
    let hops = match lease.held::<WebLease>() {
        Some(held) => hops_for(shared, service, svc, held),
        None => shared.ranked(service, svc),
    };
    let (picked, before) = shared.held_first(hops);
    if let [only] = picked.hops.as_slice()
        && before >= client_limit(&settings, only.name())
    {
        return Err(Refused::Full);
    }
    let admitted = admit_pooled(shared, service, svc, from.addr, picked).map_err(|r| match r {
        Refusal::Full => Refused::Full,
        Refusal::NoClient => Refused::NoClient,
    })?;
    let (ours, theirs) = burrow_web::pipe();
    theirs.set_reset(true);
    let ending = burrow_web::Io::ending(&theirs).ok_or(Refused::Gone)?;
    let entry = Entry {
        peer: from.addr,
        local: Some(from.local),
        index: 0,
        door: Door::Pool(ending.clone()),
    };
    let shape = Shape::of(svc, &entry);
    let incoming = crate::forward::Incoming::Web(theirs);
    let (shared, service) = (shared.clone(), service.to_string());
    tokio::spawn(async move {
        visit(shared, service, shape, incoming, entry, admitted).await;
        ending.end();
    });
    Ok(Box::new(ours))
}

pub(in crate::server) fn keeps(shared: &Shared, service: &str, from: &Visitor, ask: Ask) -> bool {
    shared.source_allowed(service, from.addr.ip())
        && match ask {
            Ask::Site => shared.still_on_web(service, &from.host).is_ok(),
            Ask::Auth => true,
        }
}
