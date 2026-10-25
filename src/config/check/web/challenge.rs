use std::net::SocketAddr;

use super::Spans;
use crate::config::diagnostics::Diagnostic;
use crate::config::tree::Span;
use burrow_config::{ChallengeKind, ServerConfig, SiteTls, TlsMode};
use burrow_syntax::web::{Challenge, CheckPorts, NoCheck, checks_for, dns_entry_for};

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

pub(super) struct Dns<'a> {
    entries: Vec<&'a [String]>,
}

impl<'a> Dns<'a> {
    pub fn of(cfg: &'a ServerConfig) -> Dns<'a> {
        Dns {
            entries: cfg
                .web
                .iter()
                .flat_map(|w| w.letsencrypt.iter())
                .flat_map(|l| l.dns.iter().map(|e| e.domains.as_slice()))
                .collect(),
        }
    }

    pub fn covers(&self, name: &str) -> bool {
        dns_entry_for(self.entries.iter().copied().enumerate(), name).is_some()
    }
}

pub(super) fn check(
    who: Who<'_>,
    host: &str,
    ports: (Option<SocketAddr>, Option<SocketAddr>),
    dns: &Dns<'_>,
    kind: Option<ChallengeKind>,
    at: Span,
) -> Option<Diagnostic> {
    let err = checks_for(host, ports_of(ports), dns.covers(host), choice(kind)).err()?;
    let key = who.key();
    let wild = || {
        format!(
            "for example certificates: [{{hosts: [\"{host}\"], cert: certs/wild.pem, key: certs/wild.key}}] under web"
        )
    };
    let (code, message, help) = match (err, who) {
        (NoCheck::NoPort, _) => (
            "no-check",
            match who {
                Who::Site(name) => format!(
                    "{host} (service \"{name}\") cannot be checked by Let's Encrypt: open http: 80, add challenge: dns, or list a file for it in web.certificates"
                ),
                Who::Names => format!(
                    "{host} (web.redirect_names) cannot be checked by Let's Encrypt: open http: 80, or list a file for it in web.certificates"
                ),
                Who::Tunnel => format!(
                    "websocket.host \"{host}\" cannot be checked: open http: 80, set websocket.challenge: dns, or give websocket.tls files"
                ),
            },
            FORWARDING.to_string(),
        ),
        (NoCheck::DnsNotSet, _) => {
            return Some(no_dns(who, host, kind, !dns.entries.is_empty()).at(at));
        }
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
                "{key}: {}: \"{host}\" is a wildcard; Let's Encrypt issues wildcards only through the DNS check; use challenge: dns",
                kind_word(kind)
            ),
            wild(),
        ),
    };
    Some(Diagnostic::error(code, message).at(at).help(help))
}

fn kind_word(kind: Option<ChallengeKind>) -> &'static str {
    match kind {
        Some(ChallengeKind::Http) => "http",
        Some(ChallengeKind::Tls) => "tls",
        _ => "dns",
    }
}

fn no_dns(who: Who<'_>, host: &str, kind: Option<ChallengeKind>, some: bool) -> Diagnostic {
    let wildcard = burrow_syntax::web::is_wildcard(host);
    let field = match who {
        Who::Site(name) => format!("services.{name}.web.hosts"),
        Who::Names => "web.redirect_names".into(),
        Who::Tunnel => "websocket.host".into(),
    };
    let why = if wildcard {
        "Let's Encrypt gives wildcard certificates only through DNS"
    } else {
        "challenge: dns"
    };
    if !some {
        let (code, message) = match who {
            Who::Tunnel => (
                "challenge-dns",
                "websocket.challenge: dns needs web.letsencrypt.dns: the provider that sets the DNS record".to_string(),
            ),
            _ if wildcard && kind.is_none() => (
                "wildcard-needs-dns",
                format!("{field}: \"{host}\" needs DNS-01 ({why}): add web.letsencrypt.dns, or a certificate file under web.certificates or tls"),
            ),
            _ => (
                "challenge-dns",
                format!("{field}: \"{host}\" needs DNS-01 ({why}): add web.letsencrypt.dns, or use challenge: http (port 80) or tls (port 443), or give a certificate file"),
            ),
        };
        return Diagnostic::error(code, message)
            .help("for example dns: [{domains: [example.com], cloudflare: {token: \"${CF_TOKEN}\"}}] under web.letsencrypt");
    }
    let bare = host.strip_prefix("*.").unwrap_or(host);
    let domain = if wildcard {
        bare.to_string()
    } else {
        bare.split_once('.')
            .map_or(bare, |(_, rest)| rest)
            .to_string()
    };
    let what = if wildcard {
        "a wildcard"
    } else {
        "challenge: dns"
    };
    let help = match (who, wildcard) {
        (Who::Tunnel, _) => format!(
            "add {domain} to a letsencrypt.dns entry's domains; or remove websocket.challenge if the name is reachable from the internet; or give websocket.tls files"
        ),
        (_, true) => format!(
            "add {domain} to a letsencrypt.dns entry's domains, or give a wildcard file in web.certificates"
        ),
        (_, false) => format!(
            "add {domain} to a letsencrypt.dns entry's domains; or, if this name is reachable from the internet, use challenge: http (port 80) or challenge: tls (port 443); or give a certificate file for it in web.certificates"
        ),
    };
    Diagnostic::error(
        "dns-uncovered",
        format!("{field}: \"{host}\" needs a DNS check ({what}), but no web.letsencrypt.dns entry lists {domain} (or a domain above it)"),
    )
    .help(help)
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
            Some(SiteTls::Mode(TlsMode::Internal)) => {
                "the local CA signs its certificates, so Let's Encrypt never checks it (tls: internal)"
            }
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
