use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use super::listen::WsListener;
use super::{
    BIND_RETRY, DRAIN_TIMEOUT, MetricsTask, Server, WsTask, accept_tcp, bind_metrics, bind_service,
    listen, log_service, spawn_service,
};
use crate::config::settings::ServerSettings;
use crate::wire::protocol::{ByeReason, ServerMsg};

impl Server {
    pub async fn run(
        mut self,
        mut reload: mpsc::Receiver<ServerSettings>,
        mut stop: watch::Receiver<bool>,
    ) {
        let s = self.shared.settings();
        if s.insecure_plaintext {
            warn!("insecure_plaintext is enabled: forwarded TCP data is NOT encrypted");
        }
        if let Some(addr) = self.local_addr() {
            info!("listening for clients on {addr}");
        }
        if let Some(ws) = &self.websocket {
            info!(
                "listening for WebSocket clients on {}",
                ws.listener.url_hint()
            );
        }
        if let Some(url) = s.websocket.as_ref().and_then(|w| w.shared_url()) {
            info!("listening for WebSocket clients on {url}, beside the websites");
        }
        self.log_web();
        if let Some(m) = &self.metrics {
            info!("metrics on http://{}/metrics", m.addr);
        }
        for (name, svc) in &s.services {
            log_service(name, svc, self.services.get(name).and_then(|t| t.addr));
        }
        tokio::spawn(super::geo::keep(self.shared.clone()));
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.tick().await;
        let mut retry = tokio::time::interval(BIND_RETRY);
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        retry.tick().await;
        loop {
            tokio::select! {
                r = accept_tcp(&self.listener) => match r {
                    Ok((stream, peer)) => {
                        let Some(permit) = listen::try_permit(&self.shared, peer) else {
                            continue;
                        };
                        let shared = self.shared.clone();
                        tokio::spawn(async move {
                            listen::handle_tcp(shared, stream, peer, permit).await;
                        });
                    }
                    Err(e) if crate::wire::transport::tcp::gone_before_accept(&e) => {
                        debug!("a connection was gone before it was accepted: {e}");
                    }
                    Err(e) => {
                        static FAILED: crate::policy::throttle::Throttle = crate::policy::throttle::Throttle::new();
                        if let Some(more) = FAILED.allow() {
                            warn!("accept failed: {e}{more}");
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
                Some(next) = reload.recv() => self.apply(next).await,
                _ = retry.tick() => self.bind_waiting().await,
                _ = tick.tick() => {
                    let n = self.shared.rejected.swap(0, Ordering::Relaxed);
                    if n > 0 {
                        warn!("rejected {n} connections in the last minute (failed handshakes or over limits)");
                    }
                    #[cfg(feature = "web")]
                    self.shared.certs.warn_expiring();
                }
                _ = stop.changed() => break,
            }
        }
        self.shutdown().await;
    }

    async fn bind_waiting(&mut self) {
        self.move_waiting().await;
        self.refresh_self();
        let settings = self.shared.settings();
        for (name, svc) in &settings.services {
            if svc.bind.is_none() || self.services.contains_key(name) {
                continue;
            }
            if let Ok(bound) = bind_service(name, svc).await {
                let t = spawn_service(&self.shared, name, svc, bound);
                info!("service \"{name}\" could listen now");
                log_service(name, svc, t.addr);
                self.services.insert(name.clone(), t);
            }
        }
    }

    async fn move_waiting(&mut self) {
        let settings = self.shared.settings();
        let mut next = (*settings).clone();
        if let Some(want) = self.wanted.websocket.clone()
            && settings
                .websocket
                .as_ref()
                .is_none_or(|w| w.listen != want.listen)
            && let Ok(l) = WsListener::bind(&want)
        {
            self.stop_websocket().await;
            let t = WsTask::start(&self.shared, l);
            info!(
                "websocket listener could listen now: {}",
                t.listener.url_hint()
            );
            self.websocket = Some(t);
            next.websocket = Some(want);
            self.shared.close_websocket_clients();
        }
        if let Some(want) = self.wanted.metrics
            && settings.metrics != Some(want)
            && let Ok(l) = bind_metrics(want)
        {
            if let Some(old) = self.metrics.take() {
                old.task.abort();
                let _ = old.task.await;
            }
            let m = MetricsTask::start(&self.shared, l);
            info!("metrics could listen now: http://{}/metrics", m.addr);
            self.metrics = Some(m);
            next.metrics = Some(want);
        }
        let web_moved = Box::pin(self.web_waiting(&mut next)).await;
        if web_moved && next.websocket.as_ref().is_some_and(|w| w.host.is_some()) {
            self.shared.close_websocket_clients();
        }
        if next.websocket != settings.websocket || next.metrics != settings.metrics || web_moved {
            *self.shared.settings.write().expect("settings lock") = Arc::new(next);
        }
    }

    pub(super) async fn shutdown(mut self) {
        info!("shutting down");
        let _ = self.shutdown_tx.send(true);
        self.health.abort();
        #[cfg(feature = "web")]
        {
            self.notify.abort();
            self.shared.notifier.sync(&[]);
        }
        if let Some(m) = self.metrics.take() {
            m.task.abort();
        }
        if let Some(ws) = self.websocket.take() {
            ws.task.abort();
        }
        self.stop_web().await;
        for (_, t) in self.services.drain() {
            for task in &t.tasks {
                task.abort();
            }
        }
        let sessions: Vec<_> = self
            .shared
            .sessions
            .lock()
            .expect("sessions lock")
            .drain()
            .map(|(_, s)| s)
            .collect();
        for s in sessions {
            let _ = s.control.try_send(ServerMsg::Bye {
                reason: ByeReason::Shutdown,
            });
            s.close();
        }
        let active = self.shared.active.load(Ordering::Acquire);
        if active > 0 {
            info!(
                "waiting up to {}s for {active} active connections",
                DRAIN_TIMEOUT.as_secs()
            );
            let wait = async {
                while self.shared.active.load(Ordering::Acquire) > 0 {
                    let n = self.shared.drained.notified();
                    if self.shared.active.load(Ordering::Acquire) == 0 {
                        break;
                    }
                    n.await;
                }
            };
            let _ = tokio::time::timeout(DRAIN_TIMEOUT, wait).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
