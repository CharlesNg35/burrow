use std::sync::atomic::Ordering;

use super::Shared;
use crate::metrics::{ClientStatus, HealthStatus, ServiceStatus, Status, VisitorStatus};

impl Shared {
    pub fn status(&self) -> Status {
        let settings = self.settings();
        let sessions = self.sessions.lock().expect("sessions lock").clone();
        let clients = settings
            .clients
            .keys()
            .map(|name| match sessions.get(name) {
                Some(s) => ClientStatus {
                    name: name.clone(),
                    connected: true,
                    connected_secs: Some(s.since.elapsed().as_secs()),
                    from: Some(s.peer.to_string()),
                    open: s.active.load(Ordering::Relaxed) as u64,
                    idle_channels: s.idle_count() as u64,
                    left_secs_ago: None,
                },
                None => ClientStatus {
                    name: name.clone(),
                    connected: false,
                    connected_secs: None,
                    from: None,
                    open: 0,
                    idle_channels: 0,
                    left_secs_ago: self.metrics.left_secs_ago(name),
                },
            })
            .collect();
        let services = settings
            .services
            .iter()
            .map(|(name, svc)| {
                let stats = self.metrics.service(name);
                let health = match svc.health {
                    Some(_) => svc
                        .clients
                        .iter()
                        .filter(|c| sessions.contains_key(*c))
                        .map(|c| HealthStatus {
                            client: c.clone(),
                            up: self.is_up(name, c),
                        })
                        .collect(),
                    None => Vec::new(),
                };
                ServiceStatus {
                    name: name.clone(),
                    protocol: svc.protocol.as_str().to_string(),
                    bind: svc.shown_bind(),
                    target: svc.shown_target(),
                    clients: svc.clients.clone(),
                    mode: (svc.clients.len() > 1).then(|| {
                        match svc.mode {
                            crate::config::model::Mode::Failover => "failover",
                            crate::config::model::Mode::Balance => "balance",
                        }
                        .to_string()
                    }),
                    limit: svc.bandwidth.map(crate::ratelimit::describe),
                    visitors: svc.visitors.clone(),
                    proxy_protocol: svc.proxy_protocol.map(|v| {
                        match v {
                            crate::proxy_protocol::Version::V1 => "v1",
                            crate::proxy_protocol::Version::V2 => "v2",
                        }
                        .to_string()
                    }),
                    filtered: !svc.sources.is_open(),
                    open: self.service_counts.current(name),
                    total: stats.total.load(Ordering::Relaxed),
                    refused: stats.refused.load(Ordering::Relaxed),
                    bytes_in: stats.traffic.inbound.load(Ordering::Relaxed),
                    bytes_out: stats.traffic.outbound.load(Ordering::Relaxed),
                    health,
                }
            })
            .collect();
        let visitors = settings
            .visitors
            .keys()
            .map(|name| VisitorStatus {
                name: name.clone(),
                open: self.visitor_counts.current(name),
                total: self.metrics.visitor(name).total.load(Ordering::Relaxed),
            })
            .collect();
        let mut listening = Vec::new();
        if let Some(l) = settings.listen {
            listening.push(format!("clients on tcp {l}"));
        }
        if let Some(ws) = &settings.websocket {
            let scheme = if ws.tls.is_some() { "wss" } else { "ws" };
            listening.push(format!("clients on {scheme}://{}{}", ws.listen, ws.path));
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
        }
    }
}
