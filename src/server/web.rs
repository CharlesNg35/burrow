use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use burrow_web::{
    BoxIo, Proxy, ProxySettings, Refused, Site, TunnelConn, TunnelMode, TunnelSettings, Upstream,
    Visitor,
};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

use super::listen::Permit;
use super::tcp::{Door, Entry, Refusal, admit, visit};
use super::{FREED_TRIES, Server, Shared, bind_error};
use crate::base::failure::Failure;
use crate::config::settings::ServerSettings;
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
    let mut p = ProxySettings::new(web.https);
    p.sites = web
        .sites
        .iter()
        .map(|(service, site)| Site {
            service: service.clone(),
            hosts: site.hosts.clone(),
        })
        .collect();
    p.max_pending = s.max_handshakes;
    p.handshake_timeout = s.handshake_timeout;
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

pub(super) fn bind_web(addr: SocketAddr) -> Result<TcpListener, Failure> {
    crate::wire::transport::tcp::bind_tcp(addr)
        .map_err(|e| bind_error("the web proxy's HTTPS port", Protocol::Tcp, addr, &e))
}

impl WebTask {
    pub(super) fn bind(
        shared: &Arc<Shared>,
        s: &ServerSettings,
    ) -> Result<Option<WebTask>, Failure> {
        let Some(p) = proxy_settings(s)? else {
            return Ok(None);
        };
        let listener = bind_web(p.https)?;
        Ok(Some(WebTask::start(shared, listener, p)))
    }

    fn start(shared: &Arc<Shared>, listener: TcpListener, p: ProxySettings) -> WebTask {
        let https = p.https;
        let proxy = Proxy::start(listener, p, Arc::new(Doorway(shared.clone())));
        WebTask { proxy, https }
    }

    pub(super) fn addr(&self) -> SocketAddr {
        self.proxy.local_addr()
    }

    pub(super) async fn stop(self) {
        self.proxy.stop().await;
    }
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
        let Some(p) = p else {
            if self.web.is_some() {
                self.stop_web().await;
                info!("web proxy stopped");
            }
            return;
        };
        if let Some(t) = self.web.as_ref().filter(|t| t.https == p.https) {
            let before = t.proxy.table();
            t.proxy.apply(&p);
            if *before != *t.proxy.table() {
                info!("web proxy updated");
            }
            return;
        }
        let conflicts = |a: SocketAddr| crate::config::check::binds_conflict(a, p.https);
        let freed = self
            .web
            .as_ref()
            .is_some_and(|t| conflicts(t.https) || conflicts(t.addr()))
            || released.iter().any(|a| conflicts(*a));
        let old = match self.web.as_ref() {
            Some(t) if freed => {
                let https = t.https;
                self.stop_web().await;
                Some(https)
            }
            Some(t) => Some(t.https),
            None => None,
        };
        match bind_web_with_retry(p.https, if freed { FREED_TRIES } else { 1 }).await {
            Ok(l) => {
                self.stop_web().await;
                let t = WebTask::start(&self.shared, l, p);
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
        p.https = old;
        if let Some(web) = next.web.as_mut() {
            web.https = old;
        }
        if !freed && let Some(t) = &self.web {
            t.proxy.apply(&p);
            error!("{why}; the web proxy stays on {}", t.addr());
            return;
        }
        match bind_web_with_retry(old, FREED_TRIES).await {
            Ok(l) => {
                let t = WebTask::start(&self.shared, l, p);
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
        let want = want.https;
        let Ok(Some(p)) = proxy_settings(&wanted) else {
            return false;
        };
        let Ok(l) = bind_web(want) else {
            return false;
        };
        self.stop_web().await;
        let t = WebTask::start(&self.shared, l, p);
        info!("web proxy could listen now: {}", t.addr());
        self.web = Some(t);
        *next = wanted;
        true
    }
}
