use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::auth::Access;
use crate::auth::oidc::Logins;
use crate::http::{Conn, Proto, serve};
use crate::io::{Io, Prefixed};
use crate::log::{LogFile, WARN_EVERY};
use crate::permit::{Permit, Permits};
use crate::proxy_header::read_proxy_header;
use crate::route::Table;
use crate::terminate::SiteTls;
use crate::terminate::{Picked, acceptor_for};
use crate::tls::client_auth::Verifiers;
use crate::tls::{Certificates, cert_names};
use crate::{AccessLog, LogFormat, ProxySettings, Timeouts, TunnelConn, Upstream, Visitor};

mod arrive;

use arrive::{Dispatch, STALLED, Seen, accept_loop, refuse};

pub(crate) struct Shared<U> {
    pub(crate) up: Arc<U>,
    pub(crate) certs: Arc<dyn Certificates>,
    pub(crate) https_port: Option<u16>,
    site_tls: SiteTls,
    http2: AtomicBool,
    verifiers: Verifiers,
    pub(crate) access: Access,
    pub(crate) logins: Logins,
    table: RwLock<Arc<Table>>,
    timing: RwLock<Timeouts>,
    log: RwLock<Option<(Arc<LogFile>, LogFormat)>>,
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
        let table = Table::build(&settings);
        let verifiers = Verifiers::new(certs.clone());
        verifiers.refresh(table.client_auths());
        let shared = Arc::new(Shared {
            up,
            site_tls: SiteTls::new(certs.clone()),
            http2: AtomicBool::new(settings.http2),
            verifiers,
            access: Access::default(),
            logins: Logins::new(settings.certs.state_dir.as_deref()),
            certs,
            https_port: https.map(|a| a.port()),
            table: RwLock::new(Arc::new(table)),
            timing: RwLock::new(settings.timeouts),
            log: RwLock::new(None),
            permits: Permits::new(settings.max_pending),
            stop: stopped,
        });
        shared.open_log(settings.access_log.as_ref());
        shared.logins.apply(&settings.oidc);
        let mut tasks = vec![tokio::spawn(sweep(shared.clone()))];
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
        let (shared, given) = (self.shared.clone(), settings.clone());
        let built = tokio::task::spawn_blocking(move || {
            let table = Table::build_after(&given, Some(&shared.table()));
            shared.verifiers.refresh(table.client_auths());
            table
        })
        .await;
        let table = match built {
            Ok(table) => table,
            Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Err(_) => return,
        };
        *self.shared.table.write().expect("table lock") = Arc::new(table);
        *self.shared.timing.write().expect("timing lock") = settings.timeouts;
        self.shared.open_log(settings.access_log.as_ref());
        self.shared.logins.apply(&settings.oidc);
        self.shared.permits.resize(settings.max_pending);
        self.shared.http2.store(settings.http2, Ordering::Release);
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

    pub fn admitted(&self) -> usize {
        self.shared.permits.admitted()
    }

    pub fn table(&self) -> Arc<Table> {
        self.shared.table()
    }

    pub fn access_notes(&self) -> Vec<(String, String)> {
        let table = self.shared.table();
        let mut out = Vec::new();
        for (key, note) in self.shared.verifiers.notes(std::time::SystemTime::now()) {
            for service in table.services_with(&key) {
                out.push((service.to_string(), note.clone()));
            }
        }
        out.sort();
        out.dedup();
        out
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

    pub(crate) fn http2(&self) -> bool {
        self.http2.load(Ordering::Acquire)
    }

    pub(crate) fn stop(&self) -> watch::Receiver<bool> {
        self.stop.clone()
    }

    fn sends_proxy_header(&self, peer: SocketAddr) -> bool {
        let table = self.table();
        crate::http::forwarded::is_trusted(&table.forwarding.proxy_protocol_from, peer.ip())
    }

    pub(crate) fn access_log(&self) -> Option<(Arc<LogFile>, LogFormat)> {
        self.log.read().expect("log lock").clone()
    }

    fn open_log(&self, wanted: Option<&AccessLog>) {
        let mut held = self.log.write().expect("log lock");
        let Some(wanted) = wanted else {
            *held = None;
            return;
        };
        if let Some((file, _)) = held.as_ref().filter(|(f, _)| f.path() == wanted.file) {
            *held = Some((file.clone(), wanted.format));
            return;
        }
        match LogFile::open(&wanted.file) {
            Ok(file) => *held = Some((Arc::new(file), wanted.format)),
            Err(e) => {
                warn!(
                    "web: cannot open the access log {}: {e}",
                    wanted.file.display()
                );
                *held = None;
            }
        }
    }

    pub(crate) fn timeouts(&self) -> Timeouts {
        *self.timing.read().expect("timing lock")
    }
}

const PROXY_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
pub const SWEEP: Duration = Duration::from_secs(60);

async fn sweep<U: Upstream>(shared: Arc<Shared<U>>) {
    let mut every = tokio::time::interval(SWEEP);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        shared.access.sweep();
        for l in shared.table().limits() {
            l.sweep();
        }
    }
}

static LAST_WARNED: AtomicU64 = AtomicU64::new(0);
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

fn warn_accept(e: &std::io::Error) {
    let now = burrow_syntax::duration::now_secs();
    let last = LAST_WARNED.load(Ordering::Relaxed);
    if now >= last + WARN_EVERY.as_secs()
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

pub(super) async fn accepted(listener: &TcpListener) -> (TcpStream, SocketAddr) {
    loop {
        match listener.accept().await {
            Ok(v) => return v,
            Err(e) if gone_before_accept(&e) => {
                debug!("web: a connection was gone before it was accepted: {e}");
            }
            Err(e) => {
                warn_accept(&e);
                tokio::time::sleep(ACCEPT_BACKOFF).await;
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
        let shared = shared.clone();
        tokio::spawn(async move {
            let proxied = shared.sends_proxy_header(peer);
            let Some((io, peer)) = after_proxy_header(stream, peer, proxied).await else {
                return;
            };
            serve(
                shared,
                Box::new(io),
                Conn::new(peer, local, None, Proto::H1),
            )
            .await;
        });
    }
}

pub(super) async fn after_proxy_header(
    mut stream: TcpStream,
    peer: SocketAddr,
    proxied: bool,
) -> Option<(Prefixed<TcpStream>, SocketAddr)> {
    if !proxied {
        return Some((Prefixed::new(Vec::new(), stream), peer));
    }
    match read_proxy_header(&mut stream, PROXY_HEADER_TIMEOUT).await {
        Ok(read) => Some((
            Prefixed::new(read.rest, stream),
            read.source.unwrap_or(peer),
        )),
        Err(e) => {
            debug!(%peer, "web: no PROXY header from a proxy_protocol_from peer: {e}");
            None
        }
    }
}

async fn dispatch<U: Upstream>(shared: &Arc<Shared<U>>, seen: Seen, mut permit: Permit) {
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
        Dispatch::Passthrough(service) => {
            drop(permit);
            let from = Visitor {
                addr: peer,
                local,
                host,
            };
            passthrough(shared.up.as_ref(), &service, &from, io).await;
        }
        Dispatch::Tunnel(path) => {
            let Some(conn) =
                start_tunnel(shared, io, (peer, local), (&host, sni), deadline, permit).await
            else {
                return;
            };
            shared.up.tunnel(conn.into_conn(path, false)).await;
        }
        Dispatch::ByPath => {
            let Some(conn) =
                start_tunnel(shared, io, (peer, local), (&host, sni), deadline, permit).await
            else {
                return;
            };
            crate::tunnel::serve_by_path(shared.up.clone(), &table, &host, conn).await;
        }
        Dispatch::Terminate => {
            let picked = Picked {
                name: &host,
                sni,
                http2: shared.http2(),
                client_auth: table.client_auth(&host),
            };
            let Some(acceptor) = acceptor_for(
                &shared.site_tls,
                shared.certs.as_ref(),
                &shared.verifiers,
                &picked,
            ) else {
                debug!(%peer, "web: no certificate for \"{host}\", closing");
                return;
            };
            let accepting = crate::terminate::accept(acceptor, io, deadline);
            permit.hello_seen();
            let Some(tls) = shared.permits.hold(&mut permit, accepting).await else {
                refuse(shared, peer, STALLED);
                return;
            };
            drop(permit);
            if let Some((tls, proto)) = tls {
                serve(
                    shared.clone(),
                    tls,
                    Conn::new(peer, local, Some(host), proto),
                )
                .await;
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
    let picked = Picked {
        name: host,
        sni,
        http2: false,
        client_auth: None,
    };
    let acceptor = acceptor_for(
        &shared.site_tls,
        shared.certs.as_ref(),
        &shared.verifiers,
        &picked,
    )?;
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
