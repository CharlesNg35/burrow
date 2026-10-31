use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;
use tracing::debug;

use super::{Shared, accepted, after_proxy_header};
use crate::Upstream;
use crate::io::Prefixed;
use crate::peek::{Peek, peek_sni};
use crate::permit::{Entry, Permit};
use crate::route::{Kind, Table};

pub(super) async fn accept_loop<U: Upstream>(shared: Arc<Shared<U>>, listener: TcpListener) {
    loop {
        let room = shared.permits.enter().await;
        let (stream, peer) = accepted(&listener).await;
        let entry = room.arrived();
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut stopped = shared.stop.clone();
            let seen = tokio::select! {
                seen = arrive(&shared, stream, peer, entry) => seen,
                _ = stopped.wait_for(|s| *s) => None,
            };
            if let Some((seen, permit)) = seen {
                Box::pin(super::dispatch(&shared, seen, permit)).await;
            }
        });
    }
}

pub(super) const STALLED: &str = "this address has too many stalled TLS handshakes";

pub(super) fn refuse<U: Upstream>(shared: &Shared<U>, peer: SocketAddr, why: &str) {
    shared.permits.count_refused();
    shared.up.refused();
    debug!(%peer, "web: {why}, closing");
}

async fn arrive<U: Upstream>(
    shared: &Shared<U>,
    stream: TcpStream,
    peer: SocketAddr,
    entry: Entry,
) -> Option<(Seen, Permit)> {
    let deadline = entry.since() + shared.timeouts().handshake;
    let local = stream.local_addr().ok()?;
    shared.up.tune(&stream);
    let proxied = shared.sends_proxy_header(peer);
    let (early, peer) = after_proxy_header(stream, peer, proxied).await?;
    let Some(mut permit) = shared.permits.take(entry, peer.ip(), deadline).await else {
        refuse(shared, peer, "no TLS handshake slot came free in time");
        return None;
    };
    let greeted = Box::pin(greet(shared, early, (peer, local), deadline));
    match shared.permits.hold(&mut permit, greeted).await {
        Some(Ok(seen)) => Some((seen, permit)),
        Some(Err(Unseen::Silent)) if permit.waited() => {
            refuse(
                shared,
                peer,
                "no TLS hello after waiting for a handshake slot",
            );
            None
        }
        Some(Err(_)) => None,
        None => {
            refuse(shared, peer, STALLED);
            None
        }
    }
}

pub(super) enum Dispatch {
    Passthrough(String),
    Tunnel(String),
    ByPath,
    Terminate,
}

impl Dispatch {
    fn of(kind: &Kind) -> Dispatch {
        match kind {
            Kind::Passthrough(service) => Dispatch::Passthrough(service.clone()),
            Kind::Tunnel { path } => Dispatch::Tunnel(path.clone()),
            k if k.only_tunnel() => Dispatch::ByPath,
            Kind::Http { .. } | Kind::Plain { .. } => Dispatch::Terminate,
        }
    }
}

pub(super) struct Seen {
    pub(super) io: Prefixed<TcpStream>,
    pub(super) peer: SocketAddr,
    pub(super) local: SocketAddr,
    pub(super) host: String,
    pub(super) sni: bool,
    pub(super) kind: Dispatch,
    pub(super) table: Arc<Table>,
    pub(super) deadline: Instant,
}

enum Unseen {
    Silent,
    Closed,
}

async fn greet<U: Upstream>(
    shared: &Shared<U>,
    mut early: Prefixed<TcpStream>,
    (peer, local): (SocketAddr, SocketAddr),
    deadline: Instant,
) -> Result<Seen, Unseen> {
    let hello = match peek_sni(
        &mut early,
        deadline.saturating_duration_since(Instant::now()),
    )
    .await
    {
        Ok(Peek::Hello(h)) => h,
        Ok(other) => {
            debug!(%peer, "web: no TLS hello ({other:?}), closing");
            return Err(Unseen::Closed);
        }
        Err(e) => {
            debug!(%peer, "web: no TLS hello: {e}");
            let silent = e.kind() == std::io::ErrorKind::TimedOut;
            return Err(if silent {
                Unseen::Silent
            } else {
                Unseen::Closed
            });
        }
    };
    let (left, stream) = early.into_parts();
    let mut bytes = hello.bytes;
    bytes.extend_from_slice(&left);
    if crate::terminate::wants_challenge(&hello.alpn)
        && let Some(key) = hello
            .name
            .as_deref()
            .and_then(|n| shared.certs.alpn_challenge(n))
    {
        debug!(%peer, name = ?hello.name, "web: answering a TLS-ALPN-01 challenge");
        crate::terminate::answer_challenge(key, stream, bytes, deadline).await;
        return Err(Unseen::Closed);
    }
    let table = shared.table();
    let Some((host, kind)) = table.pick(hello.name.as_deref(), local.ip()) else {
        debug!(%peer, name = ?hello.name, "web: unknown name, closing");
        return Err(Unseen::Closed);
    };
    let kind = Dispatch::of(kind);
    Ok(Seen {
        io: Prefixed::new(bytes, stream),
        peer,
        local,
        sni: hello.name.is_some(),
        host,
        kind,
        table,
        deadline,
    })
}
