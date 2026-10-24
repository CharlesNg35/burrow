use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use burrow_web::{
    AcmeSettings, BoxIo, CertSettings, CertSource, Proxy, ProxySettings, Refused, SharedFile, Site,
    SiteTls, TunnelConn, TunnelMode, TunnelSettings, Upstream, Visitor,
};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

use super::listen::Permit;
use super::tcp::{Door, Entry, Refusal, admit, visit};
use super::{FREED_TRIES, Server, Shared, bind_error};
use crate::base::failure::Failure;
use crate::config::settings::{ServerSettings, WebSettings, WebTls};
use burrow_config::Protocol;

pub(super) struct WebTask {
    proxy: Proxy<Doorway>,
    https: SocketAddr,
}

pub(super) struct Doorway(Arc<Shared>);

impl Upstream for Doorway {
    type Permit = Permit;

    fn tune(&self, stream: &TcpStream) {
        crate::wire::transport::tcp::tune(stream, &self.0.settings().tcp);
    }

    fn refused(&self) {
        self.0
            .rejected
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.0
            .metrics
            .handshakes_refused
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    async fn open(&self, service: &str, from: &Visitor) -> Result<BoxIo, Refused> {
        let shared = &self.0;
        let settings = shared.settings();
        let svc = settings
            .services
            .get(service)
            .cloned()
            .ok_or(Refused::Gone)?;
        shared
            .still_on_web(service, &from.host)
            .map_err(|_| Refused::Gone)?;
        if !shared.source_allowed(service, from.addr.ip()) {
            shared.metrics.refuse(service);
            debug!(peer = %from.addr, "service \"{service}\": source address not allowed, closing");
            return Err(Refused::NotAllowed);
        }
        let admitted = admit(shared, service, &svc, from.addr, 0).map_err(|r| match r {
            Refusal::Full => Refused::Full,
            Refusal::NoClient => Refused::NoClient,
        })?;
        let (ours, theirs) = burrow_web::pipe();
        theirs.set_reset(true);
        let entry = Entry {
            peer: from.addr,
            local: Some(from.local),
            index: 0,
            door: Door::Web(from.host.clone()),
        };
        tokio::spawn(visit(
            shared.clone(),
            service.to_string(),
            svc,
            crate::forward::Incoming::Web(theirs),
            entry,
            admitted,
        ));
        Ok(Box::new(ours))
    }

    fn tunnel_permit(&self, peer: SocketAddr) -> Option<Permit> {
        super::listen::try_permit(&self.0, peer)
    }

    async fn tunnel(&self, conn: TunnelConn<Permit>) {
        super::listen::handle_web_tunnel(self.0.clone(), conn).await;
    }
}

pub(super) fn proxy_settings(s: &ServerSettings) -> Result<Option<ProxySettings>, Failure> {
    let Some(web) = &s.web else {
        return Ok(None);
    };
    let mut p = ProxySettings {
        https: web.https,
        http: web.http,
        ..ProxySettings::default()
    };
    p.sites = web
        .sites
        .iter()
        .map(|(service, site)| Site {
            service: service.clone(),
            hosts: site.hosts.clone(),
            tls: site_tls(&site.tls),
            https_redirect: site.https_redirect,
        })
        .collect();
    p.max_pending = s.max_handshakes;
    p.timeouts.handshake = s.handshake_timeout;
    p.timeouts.request_header = web.request_header_timeout;
    p.timeouts.idle = web.idle_timeout;
    p.certs = cert_settings(s, web);
    if let Some(ws) = s.websocket.as_ref()
        && let Some(host) = &ws.host
    {
        let route = super::listen::WsRoute::new(ws)?;
        p.tunnel = Some(TunnelSettings {
            host: host.clone(),
            path: ws.path.clone(),
            mode: TunnelMode::OwnName,
            tls: route.acceptor(),
        });
    }
    Ok(Some(p))
}

fn site_tls(tls: &WebTls) -> SiteTls {
    match tls {
        WebTls::Passthrough => SiteTls::Passthrough,
        WebTls::Auto => SiteTls::Https(CertSource::Auto),
        WebTls::Files { cert, key } => SiteTls::Https(CertSource::OwnFile {
            cert: cert.clone(),
            key: key.clone(),
        }),
        WebTls::Off => SiteTls::Plain,
    }
}

fn cert_settings(s: &ServerSettings, web: &WebSettings) -> CertSettings {
    let tunnel_file = s
        .websocket
        .as_ref()
        .filter(|ws| ws.host.is_some())
        .and_then(|ws| ws.tls.as_ref())
        .map(|t| (t.cert_path.clone(), t.key_path.clone()));
    CertSettings {
        shared: web
            .certificates
            .iter()
            .map(|c| SharedFile {
                hosts: c.hosts.clone(),
                cert: c.cert.clone(),
                key: c.key.clone(),
            })
            .collect(),
        tunnel_file,
        acme: web.letsencrypt.as_ref().map(|l| AcmeSettings {
            directory_url: l.directory.clone(),
            ca: l.ca.clone(),
            email: l.email.clone(),
            profile: None,
            state_dir: None,
            challenges: std::iter::once(burrow_web::tls::CertCheck::Tls)
                .chain(web.http.map(|_| burrow_web::tls::CertCheck::Http))
                .collect(),
        }),
    }
}

pub(super) fn bind_web(addr: SocketAddr) -> Result<TcpListener, Failure> {
    crate::wire::transport::tcp::bind_tcp(addr)
        .map_err(|e| bind_error("the web proxy's HTTPS port", Protocol::Tcp, addr, &e))
}

impl WebTask {
    pub(super) async fn bind(
        shared: &Arc<Shared>,
        s: &ServerSettings,
    ) -> Result<Option<WebTask>, Failure> {
        let Some((p, https)) = with_https(proxy_settings(s)?) else {
            return Ok(None);
        };
        let listener = bind_web(https)?;
        Ok(Some(WebTask::start(shared, listener, p).await))
    }

    async fn start(shared: &Arc<Shared>, listener: TcpListener, p: ProxySettings) -> WebTask {
        let https = p.https.unwrap_or_else(|| listener_addr(&listener));
        let listeners = burrow_web::Listeners {
            https: Some(listener),
            http: None,
        };
        let up = Arc::new(Doorway(shared.clone()));
        let proxy = Proxy::start(listeners, p, up, burrow_web::tls::Store::new()).await;
        WebTask { proxy, https }
    }

    pub(super) fn addr(&self) -> SocketAddr {
        self.proxy.https_addr().unwrap_or(self.https)
    }

    pub(super) async fn stop(self) {
        self.proxy.stop().await;
    }
}

fn with_https(p: Option<ProxySettings>) -> Option<(ProxySettings, SocketAddr)> {
    p.and_then(|p| p.https.map(|h| (p, h)))
}

fn listener_addr(l: &TcpListener) -> SocketAddr {
    l.local_addr()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)))
}

async fn bind_web_with_retry(addr: SocketAddr, tries: usize) -> Result<TcpListener, Failure> {
    let mut last = None;
    for _ in 0..tries {
        match bind_web(addr) {
            Ok(l) => return Ok(l),
            Err(f) => last = Some(f),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(last.expect("at least one attempt"))
}

impl Server {
    pub fn web_addr(&self) -> Option<SocketAddr> {
        self.web.as_ref().map(WebTask::addr)
    }

    pub(super) fn log_web(&self) {
        if let (Some(t), Some(web)) = (&self.web, &self.shared.settings().web) {
            let names: usize = web.sites.values().map(|s| s.hosts.len()).sum();
            info!(
                "web proxy on {} ({names} {} passed through{})",
                t.addr(),
                crate::config::diagnostics::plural(names, "name"),
                web.tunnel
                    .as_ref()
                    .map_or_else(String::new, |h| format!("; the tunnel on {h}"))
            );
        }
    }

    pub(super) async fn stop_web(&mut self) {
        if let Some(t) = self.web.take() {
            t.stop().await;
        }
    }

    pub(super) async fn apply_web(&mut self, next: &mut ServerSettings, released: &[SocketAddr]) {
        let p = match proxy_settings(next) {
            Ok(p) => p,
            Err(f) => {
                error!("{}; the web proxy keeps its current settings", f.message);
                next.web = self.shared.settings().web.clone();
                return;
            }
        };
        let Some((p, https)) = with_https(p) else {
            if self.web.is_some() {
                self.stop_web().await;
                info!("web proxy stopped");
            }
            return;
        };
        if let Some(t) = self.web.as_ref().filter(|t| t.https == https) {
            let before = t.proxy.table();
            t.proxy.apply(&p).await;
            if *before != *t.proxy.table() {
                info!("web proxy updated");
            }
            return;
        }
        let conflicts = |a: SocketAddr| crate::config::check::binds_conflict(a, https);
        let freed = self
            .web
            .as_ref()
            .is_some_and(|t| conflicts(t.https) || conflicts(t.addr()))
            || released.iter().any(|a| conflicts(*a));
        let old = match self.web.as_ref() {
            Some(t) if freed => {
                let was = t.https;
                self.stop_web().await;
                Some(was)
            }
            Some(t) => Some(t.https),
            None => None,
        };
        match bind_web_with_retry(https, if freed { FREED_TRIES } else { 1 }).await {
            Ok(l) => {
                self.stop_web().await;
                let t = WebTask::start(&self.shared, l, p).await;
                info!("web proxy now on {}", t.addr());
                self.web = Some(t);
            }
            Err(f) => self.keep_web(next, old, freed, p, &f.message).await,
        }
    }

    async fn keep_web(
        &mut self,
        next: &mut ServerSettings,
        old: Option<SocketAddr>,
        freed: bool,
        mut p: ProxySettings,
        why: &str,
    ) {
        let Some(old) = old else {
            error!(
                "{why}; the web proxy stays off and tries again every {}s",
                super::BIND_RETRY.as_secs()
            );
            next.web = None;
            return;
        };
        p.https = Some(old);
        if let Some(web) = next.web.as_mut() {
            web.https = Some(old);
        }
        if !freed && let Some(t) = &self.web {
            t.proxy.apply(&p).await;
            error!("{why}; the web proxy stays on {}", t.addr());
            return;
        }
        match bind_web_with_retry(old, FREED_TRIES).await {
            Ok(l) => {
                let t = WebTask::start(&self.shared, l, p).await;
                error!("{why}; the web proxy stays on {}", t.addr());
                self.web = Some(t);
            }
            Err(f) => {
                error!(
                    "{why}, and its previous address is no longer free either ({}); websites cannot be reached until the config is fixed or the server restarts",
                    f.message
                );
                next.web = None;
            }
        }
    }

    pub(super) async fn web_waiting(&mut self, next: &mut ServerSettings) -> bool {
        let Some(want) = self.wanted.web.clone() else {
            return false;
        };
        if next.web.as_ref().is_some_and(|w| w.https == want.https) && self.web.is_some() {
            return false;
        }
        let mut wanted = next.clone();
        wanted.web = Some(want.clone());
        let Some(want) = want.https else {
            return false;
        };
        let Ok(Some(p)) = proxy_settings(&wanted) else {
            return false;
        };
        let Ok(l) = bind_web(want) else {
            return false;
        };
        self.stop_web().await;
        let t = WebTask::start(&self.shared, l, p).await;
        info!("web proxy could listen now: {}", t.addr());
        self.web = Some(t);
        *next = wanted;
        true
    }
}
