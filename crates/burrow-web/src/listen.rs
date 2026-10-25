use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::http::{Conn, serve};
use crate::io::{Io, Prefixed};
use crate::peek::{Peek, peek_sni};
use crate::permit::{Permit, Permits};
use crate::route::{Action, Kind, Table};
use crate::tls::{Certificates, cert_names};
use crate::{ProxySettings, Timeouts, TunnelConn, Upstream, Visitor};
use tokio_rustls::TlsAcceptor;

pub(crate) struct Shared<U> {
    pub(crate) up: Arc<U>,
    pub(crate) certs: Arc<dyn Certificates>,
    pub(crate) https_port: Option<u16>,
    site_tls: TlsAcceptor,
    table: RwLock<Arc<Table>>,
    timing: RwLock<Timeouts>,
    permits: Permits,
    stop: watch::Receiver<bool>,
}

pub struct Listeners {
    pub https: Option<TcpListener>,
    pub http: Option<TcpListener>,
}

pub struct Proxy<U: Upstream> {
    shared: Arc<Shared<U>>,
    https: Option<SocketAddr>,
    http: Option<SocketAddr>,
    tasks: Vec<JoinHandle<()>>,
    stop: watch::Sender<bool>,
}

fn addr_of(l: &TcpListener) -> Option<SocketAddr> {
    l.local_addr().ok()
}

impl<U: Upstream> Proxy<U> {
    pub async fn start(
        listeners: Listeners,
        settings: ProxySettings,
        up: Arc<U>,
        certs: Arc<dyn Certificates>,
    ) -> Proxy<U> {
        let https = listeners.https.as_ref().and_then(addr_of);
        let http = listeners.http.as_ref().and_then(addr_of);
        let (stop, stopped) = watch::channel(false);
        certs.apply(&settings.certs, &cert_names(&settings)).await;
        let shared = Arc::new(Shared {
            up,
            site_tls: crate::terminate::site_acceptor(certs.clone()),
            certs,
            https_port: https.map(|a| a.port()),
            table: RwLock::new(Arc::new(Table::build(&settings))),
            timing: RwLock::new(settings.timeouts),
            permits: Permits::new(settings.max_pending),
            stop: stopped,
        });
        let mut tasks = Vec::new();
        if let Some(l) = listeners.https {
            tasks.push(tokio::spawn(accept_loop(shared.clone(), l)));
        }
        if let Some(l) = listeners.http {
            tasks.push(tokio::spawn(plain_loop(shared.clone(), l)));
        }
        Proxy {
            shared,
            https,
            http,
            tasks,
            stop,
        }
    }

    pub async fn apply(&self, settings: &ProxySettings) {
        self.shared
            .certs
            .apply(&settings.certs, &cert_names(settings))
            .await;
        *self.shared.table.write().expect("table lock") = Arc::new(Table::build(settings));
        *self.shared.timing.write().expect("timing lock") = settings.timeouts;
        self.shared.permits.resize(settings.max_pending);
    }

    pub fn https_addr(&self) -> Option<SocketAddr> {
        self.https
    }

    pub fn http_addr(&self) -> Option<SocketAddr> {
        self.http
    }

    pub fn refused(&self) -> u64 {
        self.shared.permits.refused()
    }

    pub fn table(&self) -> Arc<Table> {
        self.shared.table()
    }

    pub fn certificates(&self) -> Arc<dyn Certificates> {
        self.shared.certs.clone()
    }

    pub async fn stop(self) {
        let _ = self.stop.send(true);
        for t in &self.tasks {
            t.abort();
        }
        for t in self.tasks {
            let _ = t.await;
        }
    }
}

impl<U> Shared<U> {
    pub(crate) fn table(&self) -> Arc<Table> {
        self.table.read().expect("table lock").clone()
    }

    pub(crate) fn stop(&self) -> watch::Receiver<bool> {
        self.stop.clone()
    }

    pub(crate) fn trusted_proxies(&self) -> &[burrow_syntax::cidr::Cidr] {
        &[]
    }

    pub(crate) fn timeouts(&self) -> Timeouts {
        *self.timing.read().expect("timing lock")
    }

    fn timing(&self) -> (Duration, Duration) {
        let t = self.timeouts();
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
        warn!("web: accept on a web port failed: {e}");
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
        let (stream, peer) = accepted(&listener).await;
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

async fn accepted(listener: &TcpListener) -> (TcpStream, SocketAddr) {
    loop {
        match listener.accept().await {
            Ok(v) => return v,
            Err(e) if gone_before_accept(&e) => {
                debug!("web: a connection was gone before it was accepted: {e}");
            }
            Err(e) => {
                warn_accept(&e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn plain_loop<U: Upstream>(shared: Arc<Shared<U>>, listener: TcpListener) {
    loop {
        let (stream, peer) = accepted(&listener).await;
        let Ok(local) = stream.local_addr() else {
            continue;
        };
        shared.up.tune(&stream);
        tokio::spawn(serve(
            shared.clone(),
            Box::new(stream),
            Conn::new(peer, local, None),
        ));
    }
}

struct Seen {
    io: Prefixed<TcpStream>,
    peer: SocketAddr,
    local: SocketAddr,
    host: String,
    sni: bool,
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
    if crate::terminate::wants_challenge(&hello.alpn)
        && let Some(key) = hello
            .name
            .as_deref()
            .and_then(|n| shared.certs.alpn_challenge(n))
    {
        debug!(%peer, name = ?hello.name, "web: answering a TLS-ALPN-01 challenge");
        crate::terminate::answer_challenge(key, stream, hello.bytes, accepted + handshake).await;
        return None;
    }
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
        sni: hello.name.is_some(),
        host,
        kind,
        table,
        deadline: accepted + handshake,
    })
}

async fn dispatch<U: Upstream>(shared: &Arc<Shared<U>>, seen: Seen, permit: Permit) {
    let Seen {
        io,
        peer,
        local,
        host,
        sni,
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
            let Some(conn) =
                start_tunnel(shared, io, (peer, local), (&host, sni), deadline, permit).await
            else {
                return;
            };
            shared.up.tunnel(conn.into_conn(path, false)).await;
        }
        Kind::Http { routes, .. } if routes.iter().all(|r| r.action == Action::Tunnel) => {
            let Some(conn) =
                start_tunnel(shared, io, (peer, local), (&host, sni), deadline, permit).await
            else {
                return;
            };
            crate::tunnel::serve_by_path(shared.up.clone(), &table, &host, conn).await;
        }
        Kind::Http { .. } | Kind::Plain(_) => {
            let Some(acceptor) =
                crate::terminate::acceptor_for(&shared.site_tls, shared.certs.as_ref(), &host, sni)
            else {
                debug!(%peer, "web: no certificate for \"{host}\", closing");
                return;
            };
            let tls = crate::terminate::accept(acceptor, io, deadline).await;
            drop(permit);
            if let Some(tls) = tls {
                serve(shared.clone(), tls, Conn::new(peer, local, Some(host))).await;
            }
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
    (peer, local): (SocketAddr, SocketAddr),
    (host, sni): (&str, bool),
    deadline: Instant,
    permit: Permit,
) -> Option<Started<U::Permit>> {
    let tunnel_permit = shared.up.tunnel_permit(peer)?;
    drop(permit);
    let acceptor =
        crate::terminate::acceptor_for(&shared.site_tls, shared.certs.as_ref(), host, sni)?;
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
