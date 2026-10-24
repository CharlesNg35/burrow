use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::io::{Io, Prefixed};
use crate::peek::{Peek, peek_sni};
use crate::permit::{Permit, Permits};
use crate::route::{Kind, Table};
use crate::{ProxySettings, TunnelConn, TunnelSettings, Upstream, Visitor};

struct Timing {
    hello: Duration,
    handshake: Duration,
}

struct Shared<U> {
    up: Arc<U>,
    table: RwLock<Arc<Table>>,
    tunnel: RwLock<Option<TunnelSettings>>,
    timing: RwLock<Timing>,
    permits: Permits,
    stop: watch::Receiver<bool>,
}

pub struct Proxy<U: Upstream> {
    shared: Arc<Shared<U>>,
    addr: SocketAddr,
    task: JoinHandle<()>,
    stop: watch::Sender<bool>,
}

impl<U: Upstream> Proxy<U> {
    pub fn start(listener: TcpListener, settings: ProxySettings, up: Arc<U>) -> Proxy<U> {
        let addr = listener.local_addr().unwrap_or(settings.https);
        let (stop, stopped) = watch::channel(false);
        let shared = Arc::new(Shared {
            up,
            table: RwLock::new(Arc::new(Table::build(&settings))),
            tunnel: RwLock::new(settings.tunnel.clone()),
            timing: RwLock::new(Timing {
                hello: settings.hello_timeout,
                handshake: settings.handshake_timeout,
            }),
            permits: Permits::new(settings.max_pending),
            stop: stopped,
        });
        let task = tokio::spawn(accept_loop(shared.clone(), listener));
        Proxy {
            shared,
            addr,
            task,
            stop,
        }
    }

    pub fn apply(&self, settings: &ProxySettings) {
        *self.shared.table.write().expect("table lock") = Arc::new(Table::build(settings));
        *self.shared.tunnel.write().expect("tunnel lock") = settings.tunnel.clone();
        *self.shared.timing.write().expect("timing lock") = Timing {
            hello: settings.hello_timeout,
            handshake: settings.handshake_timeout,
        };
        self.shared.permits.resize(settings.max_pending);
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn refused(&self) -> u64 {
        self.shared.permits.refused()
    }

    pub fn table(&self) -> Arc<Table> {
        self.shared.table()
    }

    pub async fn stop(self) {
        let _ = self.stop.send(true);
        self.task.abort();
        let _ = self.task.await;
    }
}

impl<U> Shared<U> {
    fn table(&self) -> Arc<Table> {
        self.table.read().expect("table lock").clone()
    }

    fn timing(&self) -> (Duration, Duration) {
        let t = self.timing.read().expect("timing lock");
        (t.hello, t.handshake)
    }
}

static LAST_WARNED: AtomicU64 = AtomicU64::new(0);

fn warn_accept(e: &std::io::Error) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let last = LAST_WARNED.load(Ordering::Relaxed);
    if now >= last + 60
        && LAST_WARNED
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        warn!("web: accept on the HTTPS port failed: {e}");
    }
}

fn gone_before_accept(e: &std::io::Error) -> bool {
    use std::io::ErrorKind as K;
    matches!(
        e.kind(),
        K::ConnectionAborted | K::ConnectionReset | K::ConnectionRefused | K::Interrupted
    )
}

async fn accept_loop<U: Upstream>(shared: Arc<Shared<U>>, listener: TcpListener) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) if gone_before_accept(&e) => {
                debug!("web: a connection was gone before it was accepted: {e}");
                continue;
            }
            Err(e) => {
                warn_accept(&e);
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Some(permit) = shared.permits.try_take(peer.ip()) else {
            shared.up.refused();
            debug!(%peer, "web: too many connections that have not shown a TLS name yet, dropping");
            continue;
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut stopped = shared.stop.clone();
            let seen = tokio::select! {
                seen = greet(&shared, stream, peer) => seen,
                _ = stopped.wait_for(|s| *s) => None,
            };
            if let Some(seen) = seen {
                dispatch(&shared, seen, permit).await;
            }
        });
    }
}

struct Seen {
    io: Prefixed<TcpStream>,
    peer: SocketAddr,
    local: SocketAddr,
    host: String,
    kind: Kind,
    table: Arc<Table>,
    deadline: Instant,
}

async fn greet<U: Upstream>(
    shared: &Shared<U>,
    mut stream: TcpStream,
    peer: SocketAddr,
) -> Option<Seen> {
    let accepted = Instant::now();
    let local = stream.local_addr().ok()?;
    shared.up.tune(&stream);
    let (hello_limit, handshake) = shared.timing();
    let hello = match peek_sni(&mut stream, hello_limit).await {
        Ok(Peek::Hello(h)) => h,
        Ok(other) => {
            debug!(%peer, "web: no TLS hello ({other:?}), closing");
            return None;
        }
        Err(e) => {
            debug!(%peer, "web: no TLS hello: {e}");
            return None;
        }
    };
    let table = shared.table();
    let Some((host, kind)) = table.pick(hello.name.as_deref(), local.ip()) else {
        debug!(%peer, name = ?hello.name, "web: unknown name, closing");
        return None;
    };
    let kind = kind.clone();
    Some(Seen {
        io: Prefixed::new(hello.bytes, stream),
        peer,
        local,
        host,
        kind,
        table,
        deadline: accepted + handshake,
    })
}

async fn dispatch<U: Upstream>(shared: &Shared<U>, seen: Seen, permit: Permit) {
    let Seen {
        io,
        peer,
        local,
        host,
        kind,
        table,
        deadline,
    } = seen;
    match kind {
        Kind::Passthrough(service) => {
            drop(permit);
            let from = Visitor {
                addr: peer,
                local,
                host,
            };
            passthrough(shared.up.as_ref(), &service, &from, io).await;
        }
        Kind::Tunnel { path } => {
            let Some(conn) = start_tunnel(shared, io, peer, local, deadline, permit).await else {
                return;
            };
            shared.up.tunnel(conn.into_conn(path, false)).await;
        }
        Kind::Http(_) => {
            let Some(conn) = start_tunnel(shared, io, peer, local, deadline, permit).await else {
                return;
            };
            crate::tunnel::serve_by_path(shared.up.clone(), &table, &host, conn).await;
        }
    }
}

async fn passthrough<U: Upstream>(
    up: &U,
    service: &str,
    from: &Visitor,
    mut io: Prefixed<TcpStream>,
) {
    let mut to = match up.open(service, from).await {
        Ok(to) => to,
        Err(why) => {
            debug!(peer = %from.addr, "web: service \"{service}\" refused the connection: {why:?}");
            return;
        }
    };
    if let Err(e) = tokio::io::copy_bidirectional(&mut io, &mut to).await {
        debug!(peer = %from.addr, "web: passthrough to \"{service}\" ended: {e}");
        io.reset_on_close();
        to.reset_on_close();
    }
}

pub(crate) struct Started<P> {
    pub io: crate::BoxIo,
    pub peer: SocketAddr,
    pub local: SocketAddr,
    pub deadline: Instant,
    pub permit: P,
}

impl<P> Started<P> {
    pub(crate) fn into_conn(self, path: String, upgraded: bool) -> TunnelConn<P> {
        TunnelConn {
            io: self.io,
            upgraded,
            path,
            peer: self.peer,
            local: self.local,
            deadline: self.deadline,
            permit: self.permit,
        }
    }
}

async fn start_tunnel<U: Upstream>(
    shared: &Shared<U>,
    io: Prefixed<TcpStream>,
    peer: SocketAddr,
    local: SocketAddr,
    deadline: Instant,
    permit: Permit,
) -> Option<Started<U::Permit>> {
    let tunnel_permit = shared.up.tunnel_permit(peer)?;
    drop(permit);
    let acceptor = shared
        .tunnel
        .read()
        .expect("tunnel lock")
        .as_ref()
        .and_then(|t| t.tls.clone())?;
    match tokio::time::timeout_at(deadline, acceptor.accept(io)).await {
        Ok(Ok(tls)) => Some(Started {
            io: Box::new(tls),
            peer,
            local,
            deadline,
            permit: tunnel_permit,
        }),
        Ok(Err(e)) => {
            shared.up.refused();
            debug!(%peer, "web: TLS handshake for the tunnel failed: {e}");
            None
        }
        Err(_) => {
            shared.up.refused();
            debug!(%peer, "web: TLS handshake for the tunnel timed out");
            None
        }
    }
}
