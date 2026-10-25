use std::net::SocketAddr;
use std::sync::Arc;

use burrow_web::tls::Certificates;
use burrow_web::{Listeners, Proxy, ProxySettings};
use tokio::net::TcpListener;
use tracing::{error, info};

use super::{Doorway, proxy_settings};
use crate::base::failure::Failure;
use crate::config::check::binds_conflict;
use crate::config::settings::ServerSettings;
use crate::server::{BIND_RETRY, FREED_TRIES, Server, Shared, bind_error};
use burrow_admin_proto::status::PortWait;
use burrow_config::Protocol;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::server) struct Ports {
    https: Option<SocketAddr>,
    http: Option<SocketAddr>,
}

impl Ports {
    fn of(p: &ProxySettings) -> Option<Ports> {
        let ports = Ports {
            https: p.https,
            http: p.http,
        };
        (ports.https.is_some() || ports.http.is_some()).then_some(ports)
    }

    fn all(self) -> impl Iterator<Item = SocketAddr> {
        self.https.into_iter().chain(self.http)
    }

    fn clash(self, a: SocketAddr) -> bool {
        self.all().any(|b| binds_conflict(a, b))
    }

    fn set(self, p: &mut ProxySettings, s: &mut ServerSettings) {
        (p.https, p.http) = (self.https, self.http);
        if let Some(web) = s.web.as_mut() {
            (web.https, web.http) = (self.https, self.http);
        }
    }
}

pub(in crate::server) struct WebTask {
    proxy: Proxy<Doorway>,
    ports: Ports,
    bound: Ports,
}

pub(in crate::server) struct BindFailed {
    key: &'static str,
    addr: SocketAddr,
    reason: String,
    failure: Failure,
}

impl BindFailed {
    fn wait(&self, stays: String) -> PortWait {
        PortWait {
            key: self.key.to_string(),
            addr: self.addr.to_string(),
            reason: self.reason.clone(),
            stays,
            retry_secs: BIND_RETRY.as_secs(),
        }
    }
}

fn reason(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => "in use".into(),
        std::io::ErrorKind::PermissionDenied => "not allowed".into(),
        _ => e.to_string(),
    }
}

fn bind_one(
    addr: SocketAddr,
    key: &'static str,
    what: &str,
) -> Result<TcpListener, Box<BindFailed>> {
    crate::wire::transport::tcp::bind_tcp(addr).map_err(|e| {
        Box::new(BindFailed {
            key,
            addr,
            reason: reason(&e),
            failure: bind_error(what, Protocol::Tcp, addr, &e),
        })
    })
}

fn bind_ports(ports: Ports) -> Result<Listeners, Box<BindFailed>> {
    let https = ports
        .https
        .map(|a| bind_one(a, "web.https", "the web proxy's HTTPS port"))
        .transpose()?;
    let http = ports
        .http
        .map(|a| bind_one(a, "web.http", "the web proxy's HTTP port"))
        .transpose()?;
    Ok(Listeners { https, http })
}

async fn bind_with_retry(ports: Ports, tries: usize) -> Result<Listeners, Box<BindFailed>> {
    crate::base::ports::retry(tries, || async { bind_ports(ports) }).await
}

impl WebTask {
    pub(in crate::server) async fn bind(
        shared: &Arc<Shared>,
        s: &ServerSettings,
    ) -> Result<Option<WebTask>, Failure> {
        let Some(p) = proxy_settings(s)? else {
            return Ok(None);
        };
        let Some(ports) = Ports::of(&p) else {
            return Ok(None);
        };
        let listeners = bind_ports(ports).map_err(|f| f.failure)?;
        Ok(Some(
            WebTask::start(shared, listeners, p, shared.certs.clone()).await,
        ))
    }

    async fn start(
        shared: &Arc<Shared>,
        listeners: Listeners,
        p: ProxySettings,
        certs: Arc<dyn Certificates>,
    ) -> WebTask {
        let ports = Ports {
            https: p.https,
            http: p.http,
        };
        let bound = Ports {
            https: listeners.https.as_ref().and_then(|l| l.local_addr().ok()),
            http: listeners.http.as_ref().and_then(|l| l.local_addr().ok()),
        };
        let up = Arc::new(Doorway(shared.clone()));
        let proxy = Proxy::start(listeners, p, up, certs).await;
        WebTask {
            proxy,
            ports,
            bound,
        }
    }

    pub(in crate::server) fn addr(&self) -> Option<SocketAddr> {
        self.bound.https
    }

    pub(in crate::server) fn http_addr(&self) -> Option<SocketAddr> {
        self.bound.http
    }

    fn describe(&self) -> String {
        let shown: Vec<String> = [("https", self.bound.https), ("http", self.bound.http)]
            .into_iter()
            .filter_map(|(k, a)| a.map(|a| format!("{k} {a}")))
            .collect();
        shown.join(" and ")
    }

    pub(in crate::server) async fn stop(self) {
        self.proxy.stop().await;
    }
}

impl Server {
    pub fn web_addr(&self) -> Option<SocketAddr> {
        self.web.as_ref().and_then(WebTask::addr)
    }

    pub fn web_http_addr(&self) -> Option<SocketAddr> {
        self.web.as_ref().and_then(WebTask::http_addr)
    }

    pub(in crate::server) fn log_web(&self) {
        if let (Some(t), Some(web)) = (&self.web, &self.shared.settings().web) {
            let names: usize = web.sites.values().map(|s| s.hosts.len()).sum();
            info!(
                "web proxy on {} ({names} {}{})",
                t.describe(),
                crate::config::diagnostics::plural(names, "name"),
                web.tunnel
                    .as_ref()
                    .map_or_else(String::new, |h| format!("; the tunnel on {h}"))
            );
        }
    }

    pub(in crate::server) async fn stop_web(&mut self) {
        if let Some(t) = self.web.take() {
            t.stop().await;
        }
    }

    fn web_certs(&self) -> Arc<dyn Certificates> {
        self.shared.certs.clone()
    }

    fn waiting(&self, wait: Option<PortWait>) -> Option<PortWait> {
        wait.clone_into(&mut self.shared.web_wait.lock().expect("web wait lock"));
        wait
    }

    pub(in crate::server) async fn apply_web(
        &mut self,
        next: &mut ServerSettings,
        released: &[SocketAddr],
    ) -> Option<PortWait> {
        self.waiting(None);
        let p = match proxy_settings(next) {
            Ok(p) => p,
            Err(f) => {
                error!("{}; the web proxy keeps its current settings", f.message);
                next.web = self.shared.settings().web.clone();
                return None;
            }
        };
        let Some((p, ports)) = p.and_then(|p| Ports::of(&p).map(|ports| (p, ports))) else {
            if self.web.is_some() {
                self.stop_web().await;
                self.shared
                    .certs
                    .apply(&burrow_web::CertSettings::default(), &[])
                    .await;
                info!("web proxy stopped");
            }
            return None;
        };
        if let Some(t) = self.web.as_ref().filter(|t| t.ports == ports) {
            let before = t.proxy.table();
            t.proxy.apply(&p).await;
            if *before != *t.proxy.table() {
                info!("web proxy updated");
            }
            return None;
        }
        let freed = self
            .web
            .as_ref()
            .is_some_and(|t| t.ports.all().chain(t.bound.all()).any(|a| ports.clash(a)))
            || released.iter().any(|a| ports.clash(*a));
        let certs = self.web_certs();
        let old = self.web.as_ref().map(|t| t.ports);
        if freed {
            self.stop_web().await;
        }
        match bind_with_retry(ports, if freed { FREED_TRIES } else { 1 }).await {
            Ok(l) => {
                self.stop_web().await;
                let t = WebTask::start(&self.shared, l, p, certs).await;
                info!("web proxy now on {}", t.describe());
                self.web = Some(t);
                None
            }
            Err(f) => Some(self.keep_web(next, old, (freed, certs), p, &f).await),
        }
    }

    async fn keep_web(
        &mut self,
        next: &mut ServerSettings,
        old: Option<Ports>,
        (freed, certs): (bool, Arc<dyn Certificates>),
        mut p: ProxySettings,
        failed: &BindFailed,
    ) -> PortWait {
        let why = &failed.failure.message;
        let Some(old) = old else {
            error!(
                "{why}; the web proxy stays off and tries again every {}s",
                BIND_RETRY.as_secs()
            );
            next.web = None;
            return self.wait_for(failed, String::new());
        };
        old.set(&mut p, next);
        if !freed && let Some(t) = &self.web {
            t.proxy.apply(&p).await;
            error!("{why}; the web proxy stays on {}", t.describe());
            let stays = t.describe();
            return self.wait_for(failed, stays);
        }
        match bind_with_retry(old, FREED_TRIES).await {
            Ok(l) => {
                let t = WebTask::start(&self.shared, l, p, certs).await;
                error!("{why}; the web proxy stays on {}", t.describe());
                let stays = t.describe();
                self.web = Some(t);
                self.wait_for(failed, stays)
            }
            Err(f) => {
                error!(
                    "{why}, and its previous address is no longer free either ({}); websites cannot be reached until the config is fixed or the server restarts",
                    f.failure.message
                );
                next.web = None;
                self.wait_for(failed, String::new())
            }
        }
    }

    fn wait_for(&self, failed: &BindFailed, stays: String) -> PortWait {
        let wait = failed.wait(stays);
        self.waiting(Some(wait.clone()));
        wait
    }

    pub(in crate::server) async fn web_waiting(&mut self, next: &mut ServerSettings) -> bool {
        let Some(want) = self.wanted.web.clone() else {
            return false;
        };
        let same =
            |w: &crate::config::settings::WebSettings| w.https == want.https && w.http == want.http;
        if next.web.as_ref().is_some_and(same) && self.web.is_some() {
            return false;
        }
        let mut wanted = next.clone();
        wanted.web = Some(want);
        let Ok(Some(p)) = proxy_settings(&wanted) else {
            return false;
        };
        let Some(ports) = Ports::of(&p) else {
            return false;
        };
        let held = self.web.as_ref().map(|t| t.ports);
        let kept = |a: SocketAddr| {
            self.web.as_ref().is_some_and(|t| {
                t.ports
                    .all()
                    .chain(t.bound.all())
                    .any(|b| binds_conflict(a, b))
            })
        };
        let free = |a| crate::wire::transport::tcp::bind_tcp(a).is_ok();
        if !ports.all().filter(|a| !kept(*a)).all(free) {
            return false;
        }
        let freed = ports.all().any(kept);
        let certs = self.web_certs();
        if freed {
            self.stop_web().await;
        }
        match bind_with_retry(ports, if freed { FREED_TRIES } else { 1 }).await {
            Ok(l) => {
                self.stop_web().await;
                let t = WebTask::start(&self.shared, l, p, certs).await;
                info!("web proxy could listen now: {}", t.describe());
                self.web = Some(t);
                self.waiting(None);
                *next = wanted;
                true
            }
            Err(f) if freed => {
                self.keep_web(next, held, (true, certs), p, &f).await;
                true
            }
            Err(_) => false,
        }
    }
}
