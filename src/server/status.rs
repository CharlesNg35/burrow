use std::sync::atomic::Ordering;

use super::Shared;
use super::standing::Seen;
use crate::config::settings::{Service, WebTls};
use crate::metrics::{ClientStatus, HealthStatus, ServiceStatus, SetAside, Status, VisitorStatus};
use burrow_admin_proto::status::{EntryStatus, HttpCounts};
use burrow_config::SELF;

type EntryRows = (Vec<HealthStatus>, Vec<SetAside>, Vec<EntryStatus>);

impl Shared {
    fn entry_rows(&self, name: &str, svc: &Service) -> EntryRows {
        let (mut health, mut aside, mut rows) = (Vec::new(), Vec::new(), Vec::new());
        let sessions = self.connected(&svc.clients);
        let entries = svc.entries();
        for e in &entries {
            let base = svc.target_of(e);
            if entries.len() > 1 {
                rows.push(EntryStatus {
                    client: e.client.clone(),
                    target: e.target.clone(),
                    open: self.entry_open(name, &e.client, base),
                });
            }
            let Some(via) = sessions.iter().find(|v| v.name() == e.client) else {
                continue;
            };
            if svc.health.is_some() {
                health.push(HealthStatus {
                    client: e.client.clone(),
                    target: e.target.clone(),
                    up: self.is_up(name, &e.client, base),
                    reason: self.health_reason(name, &e.client, base),
                });
            }
            if entries.len() < 2 {
                continue;
            }
            let targets: Vec<String> = (0..svc.ports.max(1))
                .map(|i| svc.entry_target_at(e, i))
                .collect();
            match self.seen_of(name, via, &targets) {
                Seen::Clear => {}
                seen => aside.push(SetAside {
                    client: e.client.clone(),
                    refused: seen == Seen::Refused,
                    target: e.target.clone(),
                }),
            }
        }
        (health, aside, rows)
    }

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
                let site = settings.web.as_ref().and_then(|w| w.site_of(name));
                let (health, set_aside, entries) = self.entry_rows(name, svc);
                ServiceStatus {
                    name: name.clone(),
                    protocol: svc.protocol.wire().to_string(),
                    bind: svc.shown_bind(),
                    target: svc.shown_target(),
                    clients: svc.clients.clone(),
                    mode: svc.several().then(|| svc.mode.wire().to_string()),
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
                    entries,
                    web: site.map(|s| s.hosts.clone()).unwrap_or_default(),
                    http: site
                        .filter(|s| s.tls != WebTls::Passthrough)
                        .map(|_| HttpCounts {
                            responses: stats.http_counts(),
                        }),
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
        let listening = listening(&settings);
        let (certificates, acme) = self.certificates_of(&settings);
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
            certificates,
            acme,
            local_ca: self.local_ca_of(),
            web_waiting: self.web_wait.lock().expect("web wait lock").clone(),
        }
    }
}

fn listening(settings: &crate::config::settings::ServerSettings) -> Vec<String> {
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
        listening.extend(web.https.map(|a| format!("websites on https {a}")));
        listening.extend(web.http.map(|a| format!("websites on http {a}")));
    }
    if let Some(m) = settings.metrics {
        listening.push(format!("metrics on http://{m}/metrics"));
    }
    listening
}

impl Shared {
    #[cfg(feature = "web")]
    fn certificates_of(
        &self,
        settings: &crate::config::settings::ServerSettings,
    ) -> (
        Vec<crate::metrics::CertStatus>,
        Option<crate::metrics::AcmeStatus>,
    ) {
        self.certificate_lines(settings)
    }

    #[cfg(feature = "web")]
    fn local_ca_of(&self) -> Option<crate::metrics::LocalCaStatus> {
        self.local_ca_line()
    }

    #[cfg(not(feature = "web"))]
    fn local_ca_of(&self) -> Option<crate::metrics::LocalCaStatus> {
        None
    }

    #[cfg(not(feature = "web"))]
    fn certificates_of(
        &self,
        _: &crate::config::settings::ServerSettings,
    ) -> (
        Vec<crate::metrics::CertStatus>,
        Option<crate::metrics::AcmeStatus>,
    ) {
        (Vec::new(), None)
    }
}
