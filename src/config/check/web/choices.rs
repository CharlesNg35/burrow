use std::net::SocketAddr;

use super::Spans;
use crate::config::diagnostics::Diagnostic;
use burrow_config::{Protocol, ServerConfig, SiteTls, TlsMode};

pub(super) const PLAIN_NEEDS_HTTP: &str = "plain-needs-http";

pub(in crate::config::check) fn check_choices(
    cfg: &ServerConfig,
    (https, http): (Option<SocketAddr>, Option<SocketAddr>),
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    if cfg.web.is_none() || (https.is_none() && http.is_none()) {
        return;
    }
    for (name, svc) in &cfg.services {
        let Some(web) = svc.web.as_ref().filter(|_| svc.protocol == Protocol::Tcp) else {
            continue;
        };
        if http.is_none() && web.tls() == Some(&SiteTls::Mode(TlsMode::Off)) {
            out.push(
                Diagnostic::error(
                    PLAIN_NEEDS_HTTP,
                    format!(
                        "services.{name}.web.tls: off serves plain HTTP, but web.http is not set; add http: 80"
                    ),
                )
                .at(spans.value(&["services", name, "web", "tls"]))
                .help("for example web: {https: 443, http: 80}"),
            );
        }
    }
}
