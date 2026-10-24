use std::net::SocketAddr;

use super::Spans;
use crate::config::diagnostics::Diagnostic;
use crate::config::tree::Span;
use burrow_config::{Protocol, ServerConfig, ServiceWeb, SiteTls, TlsMode};
use burrow_syntax::web::{SingleLabel, entry_for, host_name, is_wildcard};

struct Sources<'a> {
    entries: Vec<&'a [String]>,
    letsencrypt: bool,
}

impl<'a> Sources<'a> {
    fn of(cfg: &'a ServerConfig) -> Sources<'a> {
        let web = cfg.web.as_ref();
        Sources {
            entries: web
                .map(|w| w.certificates.iter().map(|c| c.hosts.as_slice()).collect())
                .unwrap_or_default(),
            letsencrypt: web.is_some_and(|w| w.letsencrypt.is_some()),
        }
    }

    fn lists(&self, host: &str) -> bool {
        !matches!(
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
    https: Option<SocketAddr>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    check_entries(cfg, spans, out);
    if https.is_none() {
        return;
    }
    let sources = Sources::of(cfg);
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
            } else if is_wildcard(&host) {
                out.push(wildcard_needs_dns(name, &host, at));
            }
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

fn wildcard_needs_dns(name: &str, host: &str, at: Span) -> Diagnostic {
    Diagnostic::error(
        "wildcard-needs-dns",
        format!(
            "services.{name}.web.hosts: \"{host}\" needs DNS-01 (Let's Encrypt gives wildcard certificates only through DNS checks), which this build does not have yet; add a certificate file under web.certificates or tls"
        ),
    )
    .at(at)
    .help(format!(
        "for example certificates: [{{hosts: [\"{host}\"], cert: certs/wild.pem, key: certs/wild.key}}] under web"
    ))
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
    host: &str,
    own_file: bool,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let sources = Sources::of(cfg);
    if own_file || sources.letsencrypt || sources.lists(host) {
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
