use std::net::SocketAddr;

use super::Spans;
use crate::config::diagnostics::Diagnostic;
use crate::config::tree::Span;
use burrow_config::{ChallengeKind, ServerConfig, SiteTls, TlsMode};
use burrow_syntax::web::{Challenge, CheckPorts, NoCheck, checks_for};

const LATER: &str = "DNS checks come in a later phase";
const FORWARDING: &str = "Let's Encrypt checks port 443 or port 80; if a router forwards them to other ports, burrow cannot see that, so forward port 80 to web.http too, or give certificate files";

#[derive(Clone, Copy)]
pub(super) enum Who<'a> {
    Site(&'a str),
    Tunnel,
    Names,
}

impl Who<'_> {
    fn key(self) -> String {
        match self {
            Who::Site(name) => format!("services.{name}.web.challenge"),
            Who::Tunnel => "websocket.challenge".into(),
            Who::Names => "web.redirect_names".into(),
        }
    }
}

pub(super) fn choice(kind: Option<ChallengeKind>) -> Option<Challenge> {
    kind.map(|k| match k {
        ChallengeKind::Dns => Challenge::Dns,
        ChallengeKind::Http => Challenge::Http,
        ChallengeKind::Tls => Challenge::Tls,
    })
}

pub(super) fn ports_of((https, http): (Option<SocketAddr>, Option<SocketAddr>)) -> CheckPorts {
    CheckPorts {
        https: https.map(|a| a.port()),
        http: http.map(|a| a.port()),
    }
}

pub(super) fn check(
    who: Who<'_>,
    host: &str,
    ports: (Option<SocketAddr>, Option<SocketAddr>),
    kind: Option<ChallengeKind>,
    at: Span,
) -> Option<Diagnostic> {
    let err = checks_for(host, ports_of(ports), false, choice(kind)).err()?;
    let key = who.key();
    let (code, message, help) = match (err, who) {
        (NoCheck::NoPort, Who::Site(name)) => (
            "no-check",
            format!(
                "{host} (service \"{name}\") cannot be checked by Let's Encrypt: open http: 80, or list a file for it in web.certificates ({LATER})"
            ),
            FORWARDING.to_string(),
        ),
        (NoCheck::NoPort, Who::Names) => (
            "no-check",
            format!(
                "{host} (web.redirect_names) cannot be checked by Let's Encrypt: open http: 80, or list a file for it in web.certificates ({LATER})"
            ),
            FORWARDING.to_string(),
        ),
        (NoCheck::NoPort, Who::Tunnel) => (
            "no-check",
            format!(
                "websocket.host \"{host}\" cannot be checked: open http: 80, or give websocket.tls files ({LATER})"
            ),
            FORWARDING.to_string(),
        ),
        (NoCheck::DnsNotSet, Who::Site(name)) if kind.is_none() => (
            "wildcard-needs-dns",
            format!(
                "services.{name}.web.hosts: \"{host}\" needs DNS-01 (Let's Encrypt gives wildcard certificates only through DNS); {LATER}, so add a certificate file under web.certificates or tls"
            ),
            format!(
                "for example certificates: [{{hosts: [\"{host}\"], cert: certs/wild.pem, key: certs/wild.key}}] under web"
            ),
        ),
        (NoCheck::DnsNotSet, _) => (
            "challenge-dns",
            format!("{key}: dns checks \"{host}\" through a DNS record; {LATER}"),
            "use challenge: tls (port 443) or http (port 80), or give a certificate file".into(),
        ),
        (NoCheck::DnsForIp, _) => (
            "challenge-dns",
            format!(
                "{key}: \"{host}\" is an IP address; Let's Encrypt checks IP addresses only over HTTP or TLS, not DNS"
            ),
            "use challenge: tls or http, or remove challenge".into(),
        ),
        (NoCheck::NoHttpPort, _) => (
            "challenge-port",
            format!(
                "{key}: http needs web.http on port 80: Let's Encrypt fetches a file there; add http: 80, or use challenge: tls or dns"
            ),
            FORWARDING.to_string(),
        ),
        (NoCheck::NoTlsPort, _) => (
            "challenge-port",
            format!(
                "{key}: tls needs web.https on port 443: Let's Encrypt connects there; use https: 443, or use challenge: http or dns"
            ),
            FORWARDING.to_string(),
        ),
        (NoCheck::WildcardNotDns, _) => (
            "challenge-wildcard",
            format!(
                "{key}: \"{host}\" is a wildcard, and Let's Encrypt gives wildcards only through DNS checks; use challenge: dns ({LATER}), or give a wildcard file in web.certificates"
            ),
            format!(
                "for example certificates: [{{hosts: [\"{host}\"], cert: certs/wild.pem, key: certs/wild.key}}] under web"
            ),
        ),
    };
    Some(Diagnostic::error(code, message).at(at).help(help))
}

pub(super) fn unused_site(cfg: &ServerConfig, spans: &Spans, out: &mut Vec<Diagnostic>) {
    let https = cfg.web.as_ref().is_some_and(|w| w.https.is_some());
    for (name, svc) in &cfg.services {
        let Some(web) = svc.web.as_ref() else {
            continue;
        };
        let burrow_config::ServiceWeb::Site(site) = web else {
            continue;
        };
        let Some(kind) = site.challenge else {
            continue;
        };
        let at = spans.value(&["services", name, "web", "challenge"]);
        if cfg.web.is_some() && !https && web.tls() != Some(&SiteTls::Mode(TlsMode::Off)) {
            let kind = match kind {
                ChallengeKind::Dns => "dns",
                ChallengeKind::Http => "http",
                ChallengeKind::Tls => "tls",
            };
            out.push(
                Diagnostic::error(
                    super::choices::NEEDS_HTTPS,
                    super::choices::needs_https(name, "challenge", kind),
                )
                .at(at)
                .help("challenge only says how Let's Encrypt checks a site's names"),
            );
            continue;
        }
        let why = match web.tls() {
            Some(SiteTls::Mode(TlsMode::Passthrough)) => {
                "the app keeps its own certificate (tls: passthrough)"
            }
            Some(SiteTls::Mode(TlsMode::Off)) => "a site with tls: off has no HTTPS",
            Some(SiteTls::Files(_)) => "the site uses its own certificate file",
            _ => continue,
        };
        out.push(
            Diagnostic::error(
                "challenge-unused",
                format!("services.{name}.web.challenge: {why}; remove challenge"),
            )
            .at(at)
            .help("challenge only says how Let's Encrypt checks a site's names"),
        );
    }
}

pub(in crate::config::check) fn unused_tunnel(spans: &Spans, by_site: bool) -> Diagnostic {
    let message = if by_site {
        "websocket.challenge is only used when the tunnel's own name gets a Let's Encrypt certificate; set challenge on the website's service instead, the tunnel uses its certificate"
    } else {
        "websocket.challenge is only used when the tunnel's own name gets a Let's Encrypt certificate; remove it"
    };
    Diagnostic::error("challenge-unused", message)
        .at(spans.value(&["websocket", "challenge"]))
        .help("the tunnel shares web.https, has no websocket.tls files and no web.certificates entry lists its name")
}
