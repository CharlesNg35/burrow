use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use super::session::ClientSession;
use super::{
    BIND_RETRY, FREED_TRIES, MetricsTask, Server, Shared, Wanted, WsTask, bind_metrics,
    bind_with_retry, bind_ws_with_retry, fields, listen, log_service, spawn_service,
};
use crate::config::settings::ServerSettings;
use crate::config::settings::WebSocketSettings;
use crate::wire::protocol::{ByeReason, ServerMsg};
use burrow_config::Protocol;

impl Server {
    pub(super) async fn apply(&mut self, next: ServerSettings) {
        let current = self.shared.settings();
        if self.listen_change_refused(&current, &next) {
            return;
        }
        let mut next = next;
        next.listen = current.listen;
        self.wanted = Wanted {
            websocket: own_listener(&next),
            metrics: next.metrics,
            #[cfg(feature = "web")]
            web: next.web.clone(),
        };
        let changed = fields::changed(&current, &next);
        for (name, _) in changed
            .iter()
            .filter(|(_, how)| *how == fields::Applied::Restart)
        {
            warn!("\"{name}\" takes effect when the server restarts");
        }
        if next.max_handshakes != current.max_handshakes {
            *self.shared.handshakes.write().expect("handshakes lock") =
                Arc::new(Semaphore::new(next.max_handshakes));
        }
        *self.shared.settings.write().expect("settings lock") = Arc::new(next.clone());
        self.shared.sync_limits(&next);
        self.shared.prune_standing(&next);
        self.shared.reloaded.send_modify(|n| *n += 1);

        self.apply_endpoints(&current, &mut next).await;
        let mut released = self.stop_moving_services(&next).await;
        let unbound = self.bind_services(&current, &next, &mut released).await;
        log_former_private_services(&current, &next);
        self.update_sessions(&current, &next);
        self.report_reload(&changed, &next, unbound);
    }

    fn listen_change_refused(&self, current: &ServerSettings, next: &ServerSettings) -> bool {
        if next.listen != current.listen {
            let clash = current.listen_clash(next).map(|c| (c.what, c.kept));
            if let Some((what, kept)) = clash {
                let why = format!(
                    "{what} uses {kept}, which this server keeps listening on until it restarts"
                );
                error!("reload rejected: {why}; keeping the current config");
                self.shared.report_apply(Some(why), Vec::new(), Vec::new());
                return true;
            }
            let show =
                |a: Option<SocketAddr>| a.map_or_else(|| "off".to_string(), |a| a.to_string());
            warn!(
                "changing \"listen\" from {} to {} needs a restart; keeping {}",
                show(current.listen),
                show(next.listen),
                show(current.listen)
            );
        }
        false
    }

    async fn apply_endpoints(&mut self, current: &ServerSettings, next: &mut ServerSettings) {
        let before = self.tunnel_at(current);
        let mut released = Vec::new();
        let ws_first = next.web.as_ref().is_some_and(|w| {
            self.websocket_addr().is_some_and(|a| {
                w.https
                    .is_some_and(|h| crate::config::check::binds_conflict(a, h))
            })
        });
        if ws_first {
            Box::pin(self.apply_own_websocket(current, next, &mut released)).await;
        }
        if next.web.is_some() || self.web.is_some() {
            if next.web.is_none() {
                released.extend(self.web_addr().into_iter().chain(self.web_http_addr()));
            }
            Box::pin(self.apply_web(next, &released)).await;
            *self.shared.settings.write().expect("settings lock") = Arc::new(next.clone());
        }
        if !ws_first {
            Box::pin(self.apply_own_websocket(current, next, &mut released)).await;
        }
        if before != self.tunnel_at(next) {
            self.shared.close_websocket_clients();
        }
        if next.metrics != current.metrics {
            let kept = self.apply_metrics(current.metrics, next.metrics).await;
            if kept != next.metrics {
                next.metrics = kept;
                *self.shared.settings.write().expect("settings lock") = Arc::new(next.clone());
            }
        }
    }

    fn tunnel_at(
        &self,
        s: &ServerSettings,
    ) -> (Option<SocketAddr>, Option<String>, Option<String>) {
        let ws = s.websocket.as_ref();
        let shared = ws.is_some_and(|w| w.host.is_some());
        let addr = if shared {
            self.web_addr()
        } else {
            self.websocket_addr()
        };
        (
            addr,
            ws.map(|w| w.path.clone()),
            ws.and_then(|w| w.host.clone()),
        )
    }

    async fn apply_own_websocket(
        &mut self,
        current: &ServerSettings,
        next: &mut ServerSettings,
        released: &mut Vec<SocketAddr>,
    ) {
        let (now, wanted) = (own_listener(current), own_listener(next));
        if now == wanted {
            return;
        }
        if let Some(a) = self.websocket_addr() {
            released.push(a);
        }
        let kept = self
            .apply_websocket(now.as_ref(), wanted.as_ref(), released)
            .await;
        if next.websocket.as_ref().is_some_and(|w| w.host.is_some()) || kept == next.websocket {
            return;
        }
        next.websocket = kept;
        *self.shared.settings.write().expect("settings lock") = Arc::new(next.clone());
    }

    async fn stop_moving_services(
        &mut self,
        next: &ServerSettings,
    ) -> Vec<(Protocol, SocketAddr, u16)> {
        let moving: Vec<String> = self
            .services
            .iter()
            .filter(|(k, t)| {
                next.services.get(*k).is_none_or(|s| {
                    s.bind != t.service.bind
                        || s.protocol != t.service.protocol
                        || s.ports != t.service.ports
                })
            })
            .map(|(k, _)| k.clone())
            .collect();
        let mut released: Vec<(Protocol, SocketAddr, u16)> = Vec::new();
        for name in moving {
            if let Some(t) = self.services.remove(&name) {
                if let Some(b) = t.service.bind {
                    released.push((t.service.protocol, b, t.service.ports));
                }
                t.stop().await;
                match next.services.get(&name) {
                    None => info!("service \"{name}\" removed"),
                    Some(s) if s.bind.is_none() => {
                        info!("service \"{name}\" is now private");
                        log_service(&name, s, None);
                    }
                    Some(_) => {}
                }
            }
        }
        released
    }

    async fn bind_services(
        &mut self,
        current: &ServerSettings,
        next: &ServerSettings,
        released: &mut Vec<(Protocol, SocketAddr, u16)>,
    ) -> Vec<String> {
        let mut unbound: Vec<String> = Vec::new();
        for (name, svc) in &next.services {
            if svc.bind.is_none() {
                if current
                    .services
                    .get(name)
                    .is_some_and(|c| c.bind.is_none() && c != svc)
                {
                    info!("service \"{name}\" updated");
                    log_service(name, svc, None);
                } else if !current.services.contains_key(name) {
                    info!("service \"{name}\" added");
                    log_service(name, svc, None);
                }
                continue;
            }
            let same_socket = self.services.get(name).is_some_and(|t| {
                t.service.bind == svc.bind
                    && t.service.protocol == svc.protocol
                    && t.service.ports == svc.ports
            });
            if same_socket {
                let t = self.services.get_mut(name).expect("checked above");
                if t.service != *svc {
                    t.service = svc.clone();
                    let _ = t.config.send(svc.clone());
                    info!("service \"{name}\" updated");
                    log_service(name, svc, t.addr);
                }
                continue;
            }
            if let Some(old) = self.services.remove(name) {
                if let Some(b) = old.service.bind {
                    released.push((old.service.protocol, b, old.service.ports));
                }
                old.stop().await;
            }
            let freed = svc.bind.is_some_and(|b| {
                released.iter().any(|(p, a, n)| {
                    *p == svc.protocol
                        && crate::config::check::ports::ranges_conflict(*a, *n, b, svc.ports)
                })
            });
            let tries = if freed { FREED_TRIES } else { 1 };
            match bind_with_retry(name, svc, tries).await {
                Ok(bound) => {
                    let t = spawn_service(&self.shared, name, svc, bound);
                    match current.services.get(name) {
                        None => info!("service \"{name}\" added"),
                        Some(c) if c.bind.is_some() => info!("service \"{name}\" moved"),
                        Some(_) => {}
                    }
                    log_service(name, svc, t.addr);
                    self.services.insert(name.clone(), t);
                }
                Err(f) => {
                    error!("{}", f.message);
                    unbound.push(format!("\"{name}\""));
                }
            }
        }

        unbound
    }

    fn update_sessions(&self, current: &ServerSettings, next: &ServerSettings) {
        let sessions: Vec<Arc<ClientSession>> = self
            .shared
            .sessions
            .lock()
            .expect("sessions lock")
            .values()
            .cloned()
            .collect();
        let toggled = next.insecure_plaintext != current.insecure_plaintext;
        for s in sessions {
            let why = match next.clients.get(&s.name) {
                None => Some(("was removed from the config", ByeReason::Removed)),
                Some(c) if !c.secrets.contains(&s.secret) => {
                    Some(("lost its secret", ByeReason::Removed))
                }
                Some(_) if toggled => Some((
                    "has to reconnect, because insecure_plaintext changed",
                    ByeReason::PlaintextMismatch,
                )),
                Some(_) => None,
            };
            if let Some((why, reason)) = why {
                info!(
                    "client \"{}\" {why}; disconnecting it and its open connections",
                    s.name
                );
                self.shared.metrics.client_left(&s.name);
                let _ = s.control.try_send(ServerMsg::Bye { reason });
                s.close();
                self.shared
                    .sessions
                    .lock()
                    .expect("sessions lock")
                    .retain(|_, v| !Arc::ptr_eq(v, &s));
                continue;
            }
            s.reconfigure(&Shared::pool_config(next));
            let _ = s
                .control
                .try_send(ServerMsg::Services(next.services_for(&s.name)));
        }
    }

    fn report_reload(
        &mut self,
        changed: &[(&'static str, fields::Applied)],
        next: &ServerSettings,
        unbound: Vec<String>,
    ) {
        let (c, v) = (next.clients.len(), next.services.len());
        let names: Vec<&str> = changed.iter().map(|(name, _)| *name).collect();
        let what = if names.is_empty() {
            String::new()
        } else {
            format!("; changed: {}", names.join(", "))
        };
        let summary = format!(
            "config reloaded ({c} {}, {v} {}{what})",
            crate::config::diagnostics::plural(c, "client"),
            crate::config::diagnostics::plural(v, "service")
        );
        let restart: Vec<&'static str> = changed
            .iter()
            .filter(|(_, how)| *how == fields::Applied::Restart)
            .map(|(name, _)| *name)
            .collect();
        self.refresh_self();
        self.shared.report_apply(None, unbound.clone(), restart);
        if unbound.is_empty() {
            info!("{summary}");
        } else {
            warn!(
                "{summary}, but service {} could not listen; trying again every {}s",
                unbound.join(", "),
                BIND_RETRY.as_secs()
            );
        }
    }

    async fn apply_websocket(
        &mut self,
        current: Option<&WebSocketSettings>,
        next: Option<&WebSocketSettings>,
        released: &[SocketAddr],
    ) -> Option<WebSocketSettings> {
        let same_socket = self.websocket.is_some()
            && matches!((current, next), (Some(c), Some(n)) if c.listen == n.listen);
        match (next, same_socket) {
            (Some(n), true) => match listen::WsRoute::new(n) {
                Ok(route) => {
                    let t = self.websocket.as_ref().expect("checked above");
                    t.listener.set_route(route);
                    let only_tls = current.is_some_and(|c| {
                        c.listen == n.listen && c.path == n.path && c.keepalive == n.keepalive
                    });
                    if only_tls {
                        info!("websocket TLS certificate reloaded; new connections use it");
                    } else {
                        info!("websocket listener updated: {}", t.listener.url_hint());
                    }
                    Some(n.clone())
                }
                Err(f) => {
                    error!("{}; websocket keeps its current settings", f.message);
                    current.cloned()
                }
            },
            (Some(n), false) => {
                let conflicts = |a| crate::config::check::binds_conflict(a, n.listen);
                let freed = (self.websocket.is_some()
                    && (current.is_some_and(|c| conflicts(c.listen))
                        || self.websocket_addr().is_some_and(conflicts)))
                    || released.iter().any(|a| conflicts(*a));
                if freed {
                    self.stop_websocket().await;
                }
                match bind_ws_with_retry(n, if freed { FREED_TRIES } else { 1 }).await {
                    Ok(l) => {
                        self.stop_websocket().await;
                        let t = WsTask::start(&self.shared, l);
                        info!("websocket listener now on {}", t.listener.url_hint());
                        self.websocket = Some(t);
                        Some(n.clone())
                    }
                    Err(f) => self.keep_websocket(current, freed, &f.message).await,
                }
            }
            (None, _) => {
                if self.websocket.is_some() {
                    self.stop_websocket().await;
                    info!("websocket listener stopped");
                }
                None
            }
        }
    }

    pub(super) async fn stop_websocket(&mut self) {
        if let Some(old) = self.websocket.take() {
            old.task.abort();
            let _ = old.task.await;
        }
    }

    async fn keep_websocket(
        &mut self,
        current: Option<&WebSocketSettings>,
        freed: bool,
        why: &str,
    ) -> Option<WebSocketSettings> {
        let Some(c) = current.filter(|_| freed || self.websocket.is_some()) else {
            error!("{why}; websocket stays off");
            return None;
        };
        if !freed {
            error!(
                "{why}; websocket stays on {}",
                self.websocket
                    .as_ref()
                    .map_or_else(String::new, |t| t.listener.url_hint())
            );
            return Some(c.clone());
        }
        match bind_ws_with_retry(c, FREED_TRIES).await {
            Ok(l) => {
                let t = WsTask::start(&self.shared, l);
                error!("{why}; websocket stays on {}", t.listener.url_hint());
                self.websocket = Some(t);
                Some(c.clone())
            }
            Err(f) => {
                error!(
                    "{why}, and its previous address is no longer free either ({}); websocket clients cannot connect until the config is fixed or the server restarts",
                    f.message
                );
                None
            }
        }
    }

    async fn apply_metrics(
        &mut self,
        current: Option<SocketAddr>,
        next: Option<SocketAddr>,
    ) -> Option<SocketAddr> {
        let Some(addr) = next else {
            if let Some(old) = self.metrics.take() {
                old.task.abort();
                let _ = old.task.await;
                info!("metrics endpoint stopped");
            }
            return None;
        };
        let clashes = self
            .metrics
            .as_ref()
            .is_some_and(|m| crate::config::check::binds_conflict(m.addr, addr));
        if clashes && let Some(old) = self.metrics.take() {
            old.task.abort();
            let _ = old.task.await;
        }
        let mut last = None;
        for _ in 0..if clashes { FREED_TRIES } else { 1 } {
            match bind_metrics(addr) {
                Ok(l) => {
                    if let Some(old) = self.metrics.take() {
                        old.task.abort();
                        let _ = old.task.await;
                    }
                    let m = MetricsTask::start(&self.shared, l);
                    info!("metrics now on http://{}/metrics", m.addr);
                    self.metrics = Some(m);
                    return next;
                }
                Err(f) => last = Some(f),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Some(f) = last {
            match &self.metrics {
                Some(m) => error!("{}; metrics stay on http://{}/metrics", f.message, m.addr),
                None => error!("{}", f.message),
            }
        }
        self.metrics.as_ref().and(current)
    }
}

pub(super) fn own_listener(s: &ServerSettings) -> Option<WebSocketSettings> {
    s.websocket.clone().filter(|w| w.host.is_none())
}

fn log_former_private_services(current: &ServerSettings, next: &ServerSettings) {
    for (name, svc) in &current.services {
        match next.services.get(name) {
            None if svc.bind.is_none() => info!("service \"{name}\" removed"),
            Some(n) if svc.bind.is_none() && n.bind.is_some() => {
                info!("service \"{name}\" is now public")
            }
            _ => {}
        }
    }
}
