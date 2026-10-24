use std::sync::atomic::Ordering;

use super::Shared;
use super::standing::Seen;
use crate::metrics::{ClientStatus, HealthStatus, ServiceStatus, SetAside, Status, VisitorStatus};
use burrow_config::SELF;

impl Shared {
    fn busy_of(&self, name: &str) -> ClientStatus {
        let (busy_total, at_limit) = self.metrics.busy(name);
        ClientStatus {
            busy_total,
            at_limit,
            ..ClientStatus::default()
        }
    }

    pub fn status(&self) -> Status {
        let settings = self.settings();
        let sessions = self.sessions.lock().expect("sessions lock").clone();
        let mut clients: Vec<ClientStatus> = settings
            .clients
            .keys()
            .map(|name| match sessions.get(name) {
                Some(s) => ClientStatus {
                    name: name.clone(),
                    connected: true,
                    connected_secs: Some(s.since.elapsed().as_secs()),
                    from: Some(s.peer.to_string()),
                    open: self.client_counts.current(name),
                    idle_channels: s.idle_count() as u64,
                    left_secs_ago: None,
                    local: false,
                    ..self.busy_of(name)
                },
                None => ClientStatus {
                    name: name.clone(),
                    connected: false,
                    connected_secs: None,
                    from: None,
                    open: self.client_counts.current(name),
                    idle_channels: 0,
                    left_secs_ago: self.metrics.left_secs_ago(name),
                    local: false,
                    ..self.busy_of(name)
                },
            })
            .collect();
        if settings.uses_self() {
            clients.push(ClientStatus {
                name: SELF.to_string(),
                connected: true,
                connected_secs: Some(self.metrics.uptime_secs()),
                from: None,
                open: self.client_counts.current(SELF),
                idle_channels: 0,
                left_secs_ago: None,
                local: true,
                ..self.busy_of(SELF)
            });
        }
        let services = settings
            .services
            .iter()
            .map(|(name, svc)| {
                let stats = self.metrics.service(name);
                let health = match svc.health {
                    Some(_) => self
                        .connected(&svc.clients)
                        .iter()
                        .map(|c| HealthStatus {
                            client: c.name().to_string(),
                            up: self.is_up(name, c.name()),
                        })
                        .collect(),
                    None => Vec::new(),
                };
                let set_aside = match svc.clients.len() {
                    0 | 1 => Vec::new(),
                    _ => self
                        .connected(&svc.clients)
                        .iter()
                        .filter_map(|s| match self.seen_any(name, s) {
                            Seen::Clear => None,
                            seen => Some(SetAside {
                                client: s.name().to_string(),
                                refused: seen == Seen::Refused,
                            }),
                        })
                        .collect(),
                };
                ServiceStatus {
                    name: name.clone(),
                    protocol: svc.protocol.wire().to_string(),
                    bind: svc.shown_bind(),
                    target: svc.shown_target(),
                    clients: svc.clients.clone(),
                    mode: (svc.clients.len() > 1).then(|| svc.mode.wire().to_string()),
                    bandwidth: svc.bandwidth,
                    visitors: svc.visitors.clone(),
                    proxy_protocol: svc.proxy_protocol.map(|v| v.wire().to_string()),
                    filtered: !svc.sources.is_open(),
                    open: self.service_counts.current(name),
                    total: stats.total.load(Ordering::Relaxed),
                    refused: stats.refused.load(Ordering::Relaxed),
                    bytes_in: stats.traffic.inbound.load(Ordering::Relaxed),
                    bytes_out: stats.traffic.outbound.load(Ordering::Relaxed),
                    health,
                    set_aside,
                }
            })
            .collect();
        let visitors = settings
            .visitors
            .keys()
            .map(|name| {
                let stats = self.metrics.visitor(name);
                VisitorStatus {
                    name: name.clone(),
                    open: self.visitor_counts.current(name),
                    total: stats.total.load(Ordering::Relaxed),
                    last_seen_secs: stats.last_seen_secs(),
                }
            })
            .collect();
        let mut listening = Vec::new();
        if let Some(l) = settings.listen {
            listening.push(format!("clients on tcp {l}"));
        }
        match settings.websocket.as_ref() {
            Some(ws) if ws.host.is_some() => {
                if let Some(url) = ws.shared_url() {
                    listening.push(format!("clients on {url} (the web port)"));
                }
            }
            Some(ws) => {
                let scheme = if ws.tls.is_some() { "wss" } else { "ws" };
                listening.push(format!("clients on {scheme}://{}{}", ws.listen, ws.path));
            }
            None => {}
        }
        if let Some(web) = &settings.web {
            listening.push(format!("websites on https {}", web.https));
        }
        if let Some(m) = settings.metrics {
            listening.push(format!("metrics on http://{m}/metrics"));
        }
        Status {
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: self.metrics.uptime_secs(),
            listening,
            handshakes_refused: self.metrics.handshakes_refused.load(Ordering::Relaxed),
            udp_peers: self.udp_peers() as u64,
            clients,
            services,
            visitors,
            web: crate::config::web_summary::summary(&settings),
        }
    }
}
