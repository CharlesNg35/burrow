use std::net::SocketAddr;

use super::Spans;
use crate::config::diagnostics::Diagnostic;
use crate::config::tree::Span;
use burrow_config::{Protocol, ServerConfig, ServiceWeb, SiteTls, TlsMode};

pub(super) const PLAIN_NEEDS_HTTP: &str = "plain-needs-http";
pub(super) const REDIRECT_PASSTHROUGH: &str = "redirect-passthrough";
pub(super) const REDIRECT_PLAIN: &str = "redirect-plain";
pub(super) const REDIRECT_NEEDS_HTTP: &str = "redirect-needs-http";
pub(super) const NEEDS_HTTPS: &str = "needs-https";
pub(super) const PLAIN_HTTP: &str = "plain-http";

pub(super) fn needs_https(name: &str, key: &str, choice: &str) -> String {
    format!(
        "services.{name}.web.{key}: {choice} needs web.https; without web.https every site is plain HTTP. Add https: 443, or remove {key}"
    )
}

fn https_choice(web: &ServiceWeb) -> Option<&'static str> {
    match web.tls()? {
        SiteTls::Mode(TlsMode::Auto) => Some("auto"),
        SiteTls::Mode(TlsMode::Passthrough) => Some("passthrough"),
        SiteTls::Mode(TlsMode::Off) => None,
        SiteTls::Files(_) => Some("{cert, key}"),
    }
}

fn error(code: &'static str, message: String, at: Span, help: &str) -> Diagnostic {
    Diagnostic::error(code, message).at(at).help(help)
}

pub(in crate::config::check) fn check_choices(
    cfg: &ServerConfig,
    (https, http): (Option<SocketAddr>, Option<SocketAddr>),
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    if cfg.web.is_none() || (https.is_none() && http.is_none()) {
        return;
    }
    let written = cfg.web.as_ref().is_some_and(|w| w.https.is_some());
    for (name, svc) in &cfg.services {
        let Some(web) = svc.web.as_ref().filter(|_| svc.protocol == Protocol::Tcp) else {
            continue;
        };
        let tls_at = spans.value(&["services", name, "web", "tls"]);
        if let Some(choice) = https_choice(web).filter(|_| !written) {
            out.push(error(
                NEEDS_HTTPS,
                needs_https(name, "tls", choice),
                tls_at,
                "for example web: {https: 443, http: 80}",
            ));
            continue;
        }
        let plain = match web.tls() {
            Some(SiteTls::Mode(TlsMode::Off)) => true,
            None => !written,
            _ => false,
        };
        if plain && http.is_some() {
            out.push(
                Diagnostic::warning(
                    PLAIN_HTTP,
                    format!(
                        "services.{name}: served as plain HTTP; anyone on the path can read and change its traffic"
                    ),
                )
                .at(spans.value(&["services", name, "web"]))
                .help(if written {
                    "for a site reached over the internet, remove tls: off so burrow serves it over HTTPS"
                } else {
                    "for a site reached over the internet, use HTTPS: add https: 443 under web"
                }),
            );
        }
        if http.is_none() && web.tls() == Some(&SiteTls::Mode(TlsMode::Off)) {
            out.push(error(
                PLAIN_NEEDS_HTTP,
                format!(
                    "services.{name}.web.tls: off serves plain HTTP, but web.http is not set; add http: 80"
                ),
                tls_at,
                "for example web: {https: 443, http: 80}",
            ));
        }
        redirect(cfg, name, web, http.is_some(), spans, out);
    }
}

fn redirect(
    cfg: &ServerConfig,
    name: &str,
    web: &ServiceWeb,
    http: bool,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let at = spans.value(&["services", name, "web", "https_redirect"]);
    let own = web.https_redirect();
    let https = cfg.web.as_ref().is_some_and(|w| w.https.is_some());
    let plain = match web.tls() {
        Some(SiteTls::Mode(TlsMode::Off)) => true,
        None => !https,
        _ => false,
    };
    match web.tls() {
        Some(SiteTls::Mode(TlsMode::Passthrough)) if own == Some(false) => out.push(error(
            REDIRECT_PASSTHROUGH,
            format!(
                "services.{name}.web.https_redirect: false: burrow cannot serve plain HTTP for a passthrough service (the app keeps its own TLS); only the redirect is possible"
            ),
            at,
            "remove https_redirect",
        )),
        Some(SiteTls::Mode(TlsMode::Passthrough)) => {}
        _ if plain && own == Some(true) => out.push(error(
            REDIRECT_PLAIN,
            format!(
                "services.{name}.web.https_redirect: true on a plain-HTTP site needs web.trusted_proxies: burrow can only tell which visitors used HTTPS at a proxy it trusts; trusted_proxies is not in this build yet, so set https_redirect: false"
            ),
            at,
            "remove https_redirect, or set it to false",
        )),
        _ if plain => {}
        _ => {
            let fallback = cfg.web.as_ref().and_then(|w| w.https_redirect);
            if !http && own.or(fallback) == Some(false) {
                let at = if own.is_some() {
                    at
                } else {
                    spans.value(&["web", "https_redirect"])
                };
                out.push(error(
                    REDIRECT_NEEDS_HTTP,
                    format!(
                        "services.{name}.web.https_redirect: false serves plain HTTP too, but web.http is not set; add http: 80"
                    ),
                    at,
                    "for example web: {https: 443, http: 80}",
                ));
            }
        }
    }
}
