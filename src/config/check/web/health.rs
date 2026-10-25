use super::{Served, Spans};
use crate::config::diagnostics::Diagnostic;
use burrow_config::{HealthCheckSetting, Protocol, ServerConfig, ServiceWeb};
use burrow_syntax::web::{SingleLabel, host_name, is_wildcard};

pub(super) fn check_health_paths(
    cfg: &ServerConfig,
    https: bool,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    for (name, svc) in &cfg.services {
        let Some(HealthCheckSetting::Tuned(h)) = &svc.health_check else {
            continue;
        };
        let Some(path) = h.path.as_deref() else {
            continue;
        };
        let at = spans.value(&["services", name, "health_check", "path"]);
        let field = format!("services.{name}.health_check.path");
        let tcp = "use health_check: true for a check that only opens a connection";
        let refuse = |message: String| Diagnostic::error("health-path", message).at(at).help(tcp);
        let Some(web) = svc.web.as_ref().filter(|_| svc.protocol == Protocol::Tcp) else {
            out.push(refuse(format!(
                "{field}: an HTTP check asks a website's app for a path, and service \"{name}\" is not a website (no web:)"
            )));
            continue;
        };
        if cfg.web.is_none() || !super::BUILT_WITH_WEB {
            out.push(refuse(format!(
                "{field}: the web proxy is off, so service \"{name}\" is not served as a website and has no path to ask"
            )));
            continue;
        }
        if Served::of(web, https) == Served::Passthrough {
            out.push(refuse(format!(
                "{field}: the app keeps its own HTTPS (tls: passthrough), so burrow cannot ask it for a path"
            )));
            continue;
        }
        if let Err(e) = burrow_syntax::url_path::route_path(path) {
            out.push(
                Diagnostic::error("health-path", format!("{field}: {e}"))
                    .at(at)
                    .help("for example path: /healthz"),
            );
            continue;
        }
        notes(name, web, &field, at, out);
    }
}

fn notes(
    name: &str,
    web: &ServiceWeb,
    field: &str,
    at: crate::config::tree::Span,
    out: &mut Vec<Diagnostic>,
) {
    if matches!(web, ServiceWeb::Site(s) if s.target_http2) {
        out.push(
            Diagnostic::warning(
                "health-path-grpc",
                format!(
                    "{field}: gRPC apps usually do not answer a plain GET; use health_check: true (the TCP check)"
                ),
            )
            .at(at)
            .help("target_http2 is set on this site, the usual sign of a gRPC app"),
        );
    }
    let hosts: Vec<String> = web
        .hosts()
        .iter()
        .filter_map(|h| host_name(h, SingleLabel::Allowed).ok())
        .collect();
    if let Some(first) = hosts
        .first()
        .filter(|_| hosts.iter().all(|h| is_wildcard(h)))
    {
        let base = first.trim_start_matches("*.");
        out.push(
            Diagnostic::warning(
                "health-path-host",
                format!(
                    "{field}: the check asks for {base}, the base name of \"{first}\"; the app must answer that name, or list a plain name in hosts for service \"{name}\""
                ),
            )
            .at(at)
            .help("apps that check the Host header answer 400 to a name they do not know"),
        );
    }
}
