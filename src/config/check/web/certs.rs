use std::net::SocketAddr;

use super::Spans;
use super::challenge::{self, Who};
use crate::config::diagnostics::Diagnostic;
use crate::config::tree::Span;
use burrow_config::{Protocol, ServerConfig, ServiceWeb, SiteTls, TlsMode};
use burrow_syntax::web::{SingleLabel, entry_for, host_name};

struct Sources<'a> {
    entries: Vec<&'a [String]>,
    letsencrypt: bool,
    wildcards: Vec<String>,
}

impl<'a> Sources<'a> {
    fn of(cfg: &'a ServerConfig) -> Sources<'a> {
        let web = cfg.web.as_ref();
        Sources {
            entries: web
                .map(|w| w.certificates.iter().map(|c| c.hosts.as_slice()).collect())
                .unwrap_or_default(),
            letsencrypt: web.is_some_and(|w| w.letsencrypt.is_some()),
            wildcards: web
                .and_then(|w| w.letsencrypt.as_ref())
                .into_iter()
                .flat_map(|l| l.dns.iter().flat_map(|e| e.wildcards.iter()))
                .filter_map(|w| host_name(w, SingleLabel::Allowed).ok())
                .collect(),
        }
    }

    fn lists(&self, host: &str) -> bool {
        let shared = self.wildcards.iter().any(|w| {
            w == host || w.strip_prefix("*.") == Some(host) || burrow_syntax::web::covers(w, host)
        });
        shared
            || !matches!(
                entry_for(self.entries.iter().copied().enumerate(), host),
                Ok(None)
            )
    }
}

fn host_at(spans: &Spans, name: &str, web: &ServiceWeb, i: usize) -> Span {
    let i = i.to_string();
    match web {
        ServiceWeb::Host(_) => spans.value(&["services", name, "web"]),
        ServiceWeb::Hosts(_) => spans.value(&["services", name, "web", &i]),
        ServiceWeb::Site(_) => spans.value(&["services", name, "web", "hosts", &i]),
    }
}

fn takes_auto(web: &ServiceWeb) -> bool {
    matches!(web.tls(), None | Some(SiteTls::Mode(TlsMode::Auto)))
}

pub(in crate::config::check) fn check_sources(
    cfg: &ServerConfig,
    ports: (Option<SocketAddr>, Option<SocketAddr>),
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    check_entries(cfg, spans, out);
    challenge::unused_site(cfg, spans, out);
    if ports.0.is_none() {
        return;
    }
    let sources = Sources::of(cfg);
    redirect_sources(cfg, &sources, ports, spans, out);
    let dns = challenge::Dns::of(cfg);
    for (name, svc) in &cfg.services {
        let Some(web) = svc.web.as_ref().filter(|w| takes_auto(w)) else {
            continue;
        };
        if svc.protocol != Protocol::Tcp {
            continue;
        }
        for (i, text) in web.hosts().iter().enumerate() {
            let Ok(host) = host_name(text, SingleLabel::Refused) else {
                continue;
            };
            if sources.lists(&host) {
                continue;
            }
            let at = host_at(spans, name, web, i);
            if !sources.letsencrypt {
                out.push(no_certificate(name, &host, at));
                continue;
            }
            let kind = match web {
                ServiceWeb::Site(site) => site.challenge,
                _ => None,
            };
            if let Some(d) = challenge::check(Who::Site(name), &host, ports, &dns, kind, at)
                && !out
                    .iter()
                    .any(|o| o.code == d.code && o.message == d.message)
            {
                out.push(d);
            }
        }
    }
}

fn push_once(out: &mut Vec<Diagnostic>, d: Diagnostic) {
    if !out
        .iter()
        .any(|o| o.code == d.code && o.message == d.message)
    {
        out.push(d);
    }
}

fn redirect_sources(
    cfg: &ServerConfig,
    sources: &Sources<'_>,
    ports: (Option<SocketAddr>, Option<SocketAddr>),
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let mut names: Vec<(Who<'_>, String, Option<burrow_config::ChallengeKind>, Span)> = Vec::new();
    for (name, svc) in &cfg.services {
        let Some(ServiceWeb::Site(site)) = svc.web.as_ref() else {
            continue;
        };
        if svc.protocol != Protocol::Tcp
            || matches!(
                site.tls,
                Some(SiteTls::Mode(
                    TlsMode::Passthrough | TlsMode::Off | TlsMode::Internal
                ))
            )
        {
            continue;
        }
        for (i, text) in site.redirect_from.iter().enumerate() {
            let at = spans.value(&["services", name, "web", "redirect_from", &i.to_string()]);
            if let Ok(host) = host_name(text, SingleLabel::Refused) {
                names.push((Who::Site(name), host, site.challenge, at));
            }
        }
    }
    for key in cfg.web.iter().flat_map(|w| w.redirect_names.keys()) {
        let host = key.trim().split('/').next().unwrap_or_default();
        if let Ok(host) = host_name(host, SingleLabel::Refused) {
            let at = spans.key(&["web", "redirect_names", key]);
            names.push((Who::Names, host, None, at));
        }
    }
    for (who, host, kind, at) in names {
        if sources.lists(&host) {
            continue;
        }
        if !sources.letsencrypt {
            let owner = match who {
                Who::Site(name) => format!("service \"{name}\""),
                _ => "web.redirect_names".to_string(),
            };
            push_once(
                out,
                Diagnostic::error(
                    "no-certificate",
                    format!(
                        "host \"{host}\" ({owner}) has no certificate; add web.letsencrypt for automatic certificates, or an entry in web.certificates whose hosts lists it"
                    ),
                )
                .at(at)
                .help("a name that only redirects still needs a certificate for https:// visitors"),
            );
            continue;
        }
        if let Some(d) = challenge::check(who, &host, ports, &challenge::Dns::of(cfg), kind, at) {
            push_once(out, d);
        }
    }
}

fn no_certificate(name: &str, host: &str, at: Span) -> Diagnostic {
    Diagnostic::error(
        "no-certificate",
        format!(
            "host \"{host}\" (service \"{name}\") has no certificate; add web.letsencrypt for automatic certificates, tls: {{cert, key}} on service \"{name}\", or an entry in web.certificates whose hosts lists it"
        ),
    )
    .at(at)
    .help("for example letsencrypt: {email: ops@example.com} under web")
}

fn check_entries(cfg: &ServerConfig, spans: &Spans, out: &mut Vec<Diagnostic>) {
    let Some(web) = cfg.web.as_ref() else {
        return;
    };
    let mut seen: Vec<(String, usize)> = Vec::new();
    for (i, entry) in web.certificates.iter().enumerate() {
        let n = i.to_string();
        if entry.hosts.is_empty() {
            out.push(
                Diagnostic::error(
                    "certificate-hosts",
                    format!(
                        "web.certificates[{i}].hosts is required: the names this file is for, like hosts: [\"*.corp.example.com\"]"
                    ),
                )
                .at(spans.value(&["web", "certificates", &n]))
                .help("list each name or wildcard the file is used for"),
            );
        }
        for (j, text) in entry.hosts.iter().enumerate() {
            let Ok(host) = host_name(text, SingleLabel::Allowed) else {
                continue;
            };
            match seen.iter().find(|(h, _)| *h == host) {
                Some((_, first)) => out.push(
                    Diagnostic::error(
                        "certificate-hosts",
                        format!(
                            "host \"{host}\" is listed in web.certificates[{first}] and [{i}]; list it in one entry only"
                        ),
                    )
                    .at(spans.value(&["web", "certificates", &n, "hosts", &j.to_string()]))
                    .help("remove it from one of the two entries"),
                ),
                None => seen.push((host, i)),
            }
        }
    }
}

pub(in crate::config::check) fn tunnel_has_source(
    cfg: &ServerConfig,
    (host, own_file): (&str, bool),
    ports: (Option<SocketAddr>, Option<SocketAddr>),
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let sources = Sources::of(cfg);
    let kind = cfg.websocket.as_ref().and_then(|w| w.challenge);
    if own_file || sources.lists(host) {
        if kind.is_some() {
            out.push(challenge::unused_tunnel(spans, false));
        }
        return;
    }
    if sources.letsencrypt {
        let at = spans.value(&["websocket", "host"]);
        out.extend(challenge::check(
            Who::Tunnel,
            host,
            ports,
            &challenge::Dns::of(cfg),
            kind,
            at,
        ));
        return;
    }
    out.push(
        Diagnostic::error(
            "tunnel-certificate",
            format!(
                "websocket.host \"{host}\" has no certificate; add web.letsencrypt, websocket.tls, or an entry in web.certificates whose hosts lists it"
            ),
        )
        .at(spans.value(&["websocket", "host"]))
        .help("for example tls: {cert: certs/fullchain.pem, key: certs/privkey.pem} under websocket"),
    );
}
