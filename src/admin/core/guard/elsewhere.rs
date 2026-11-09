use std::net::SocketAddr;

use burrow_admin_proto::codes::NOT_THIS_MACHINE;
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_config::{MetricsConfig, ServerConfig, ServiceEntry, WebConfig, WebSocketConfig};

use crate::config::settings::ServerSettings;

fn binds(s: &ServerSettings) -> Vec<(Vec<String>, SocketAddr)> {
    let mut out: Vec<(Vec<String>, SocketAddr)> = Vec::new();
    out.extend(s.listen.map(|a| (ServerConfig::LISTEN.path(), a)));
    out.extend(s.websocket.as_ref().map(|w| {
        (
            ServerConfig::WEBSOCKET.then(WebSocketConfig::LISTEN).path(),
            w.listen,
        )
    }));
    out.extend(
        s.metrics
            .map(|a| (ServerConfig::METRICS.then(MetricsConfig::LISTEN).path(), a)),
    );
    if let Some(web) = &s.web {
        out.extend(
            web.https
                .map(|a| (ServerConfig::WEB.then(WebConfig::HTTPS).path(), a)),
        );
        out.extend(
            web.http
                .map(|a| (ServerConfig::WEB.then(WebConfig::HTTP).path(), a)),
        );
    }
    for (name, svc) in &s.services {
        out.extend(svc.bind.map(|a| (ServiceEntry::BIND.of(name), a)));
    }
    out
}

pub fn not_here(
    probe: &dyn Fn(SocketAddr) -> std::io::Result<()>,
    running: &ServerSettings,
    next: &ServerSettings,
) -> Vec<Diag> {
    let before: std::collections::HashSet<(Vec<String>, std::net::IpAddr)> = binds(running)
        .into_iter()
        .map(|(p, b)| (p, b.ip()))
        .collect();
    binds(next)
        .into_iter()
        .filter(|(path, a)| {
            !a.ip().is_unspecified()
                && !before.contains(&(path.clone(), a.ip()))
                && probe(SocketAddr::new(a.ip(), 0))
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::AddrNotAvailable)
        })
        .map(|(path, a)| Diag {
            severity: Severity::Warning,
            code: NOT_THIS_MACHINE.into(),
            message: format!(
                "{}: {} is not an address of this server",
                path.join("."),
                a.ip()
            ),
            path: Some(path),
            also: Vec::new(),
            subjects: Vec::new(),
            label: None,
            help: Some(crate::base::ports::not_here_hint()),
            line: None,
            col: None,
            end_line: None,
            end_col: None,
            before: false,
            file: None,
        })
        .collect()
}
