use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use burrow_web::tls::{Certificates, Store};
use burrow_web::{Listeners, Proxy, ProxySettings};
use tokio::net::TcpListener;
use tracing::{error, info};

use super::{Doorway, proxy_settings};
use crate::base::failure::Failure;
use crate::config::check::binds_conflict;
use crate::config::settings::ServerSettings;
use crate::server::{BIND_RETRY, FREED_TRIES, Server, Shared, bind_error};
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

fn bind_one(addr: SocketAddr, what: &str) -> Result<TcpListener, Failure> {
    crate::wire::transport::tcp::bind_tcp(addr)
        .map_err(|e| bind_error(what, Protocol::Tcp, addr, &e))
}

fn bind_ports(ports: Ports) -> Result<Listeners, Failure> {
    let https = ports
        .https
        .map(|a| bind_one(a, "the web proxy's HTTPS port"))
        .transpose()?;
    let http = ports
        .http
        .map(|a| bind_one(a, "the web proxy's HTTP port"))
        .transpose()?;
    Ok(Listeners { https, http })
}

async fn bind_with_retry(ports: Ports, tries: usize) -> Result<Listeners, Failure> {
    let mut last = None;
    for _ in 0..tries {
        match bind_ports(ports) {
            Ok(l) => return Ok(l),
            Err(f) => last = Some(f),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(last.expect("at least one attempt"))
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
        let listeners = bind_ports(ports)?;
        Ok(Some(
            WebTask::start(shared, listeners, p, Store::new()).await,
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
        self.web.as_ref().map_or_else(
            || Store::new() as Arc<dyn Certificates>,
            |t| t.proxy.certificates(),
        )
    }

    pub(in crate::server) async fn apply_web(
        &mut self,
        next: &mut ServerSettings,
        released: &[SocketAddr],
    ) {
        let p = match proxy_settings(next) {
            Ok(p) => p,
            Err(f) => {
                error!("{}; the web proxy keeps its current settings", f.message);
                next.web = self.shared.settings().web.clone();
                return;
            }
        };
        let Some((p, ports)) = p.and_then(|p| Ports::of(&p).map(|ports| (p, ports))) else {
            if self.web.is_some() {
                self.stop_web().await;
                info!("web proxy stopped");
            }
            return;
        };
        if let Some(t) = self.web.as_ref().filter(|t| t.ports == ports) {
            let before = t.proxy.table();
            t.proxy.apply(&p).await;
            if *before != *t.proxy.table() {
                info!("web proxy updated");
            }
            return;
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
            }
            Err(f) => {
                self.keep_web(next, old, (freed, certs), p, &f.message)
                    .await
            }
        }
    }

    async fn keep_web(
        &mut self,
        next: &mut ServerSettings,
        old: Option<Ports>,
        (freed, certs): (bool, Arc<dyn Certificates>),
        mut p: ProxySettings,
        why: &str,
    ) {
        let Some(old) = old else {
            error!(
                "{why}; the web proxy stays off and tries again every {}s",
                BIND_RETRY.as_secs()
            );
            next.web = None;
            return;
        };
        old.set(&mut p, next);
        if !freed && let Some(t) = &self.web {
            t.proxy.apply(&p).await;
            error!("{why}; the web proxy stays on {}", t.describe());
            return;
        }
        match bind_with_retry(old, FREED_TRIES).await {
            Ok(l) => {
                let t = WebTask::start(&self.shared, l, p, certs).await;
                error!("{why}; the web proxy stays on {}", t.describe());
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
        let Ok(l) = bind_ports(ports) else {
            return false;
        };
        let certs = self.web_certs();
        self.stop_web().await;
        let t = WebTask::start(&self.shared, l, p, certs).await;
        info!("web proxy could listen now: {}", t.describe());
        self.web = Some(t);
        *next = wanted;
        true
    }
}
