use std::net::SocketAddr;
use std::path::Path;

use super::Spans;
use super::site_keys::{bad_target, redirect_target};
use crate::config::diagnostics::Diagnostic;
use crate::policy::allowlist::Cidr;
use burrow_config::ServerConfig;
use burrow_config::{Protocol, ServiceWeb, SiteTls, TlsMode};
use burrow_syntax::url_path::route_key;
use burrow_syntax::web::{AutoName, SingleLabel, entry_for, host_name, proxy_note as note_for};

pub(super) fn check_site_wide(
    cfg: &ServerConfig,
    ports: (Option<SocketAddr>, Option<SocketAddr>),
    tunnel: Option<&str>,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let Some(web) = cfg.web.as_ref() else {
        return;
    };
    if !web.trusted_proxies.is_empty() && web.letsencrypt.is_some() && ports.0.is_some() {
        out.extend(proxy_note(cfg, ports, tunnel, spans));
    }
    ranges(
        "trusted_proxies",
        &web.trusted_proxies,
        "every visitor could set its own address with X-Forwarded-For",
        spans,
        out,
    );
    ranges(
        "proxy_protocol_from",
        &web.proxy_protocol_from,
        "every visitor would have to send a PROXY header",
        spans,
        out,
    );
    if let Some(log) = &web.access_log {
        let file = Path::new(log.file().trim());
        let folder = base_dir.join(file.parent().unwrap_or(Path::new("")));
        if log.file().trim().is_empty() {
            out.push(
                Diagnostic::error(
                    "access-log",
                    "web.access_log needs a file, like logs/access.log",
                )
                .at(spans.value(&["web", "access_log"]))
                .help("paths are relative to the config file"),
            );
        } else if !folder.is_dir() {
            out.push(
                Diagnostic::warning(
                    "access-log",
                    format!(
                        "web.access_log: the folder of {} does not exist yet",
                        file.display()
                    ),
                )
                .at(spans.value(&["web", "access_log"]))
                .help("create the folder, writable by burrow's user"),
            );
        }
    }
    let mut seen: Vec<(String, String, &str)> = Vec::new();
    for (name, to) in &web.redirect_names {
        let at = spans.key(&["web", "redirect_names", name]);
        let text = name.trim();
        let (host, path) = match text.find('/') {
            Some(i) => (&text[..i], &text[i..]),
            None => (text, "/"),
        };
        let problem = match (host_name(host, SingleLabel::Allowed), route_key(path)) {
            (Err(e), _) => Some(format!("web.redirect_names: {e}")),
            (_, Err(e)) => Some(format!("web.redirect_names: \"{text}\": {e}")),
            (Ok(host), Ok(key)) => match seen.iter().find(|(h, k, _)| *h == host && *k == key) {
                Some((_, _, first)) => Some(format!(
                    "web.redirect_names: \"{first}\" and \"{text}\" are the same route; paths match whatever the letter case"
                )),
                None => {
                    seen.push((host, key, text));
                    None
                }
            },
        };
        if let Some(m) = problem {
            out.push(Diagnostic::error("redirect-name", m).at(at).help(
                "write a name, or a name and a path, like old.example.com or example.com/shop",
            ));
        }
        if !redirect_target(to.to()) {
            out.push(
                Diagnostic::error("redirect-target", bad_target(to.to()))
                    .at(spans.value(&["web", "redirect_names", name]))
                    .help("for example https://example.com"),
            );
        }
    }
}

fn ranges(key: &str, list: &[String], open: &str, spans: &Spans, out: &mut Vec<Diagnostic>) {
    for (i, text) in list.iter().enumerate() {
        let n = i.to_string();
        let at = spans.value(&["web", key, &n]);
        match text.trim().parse::<Cidr>() {
            Err(e) => out.push(
                Diagnostic::error(
                    "invalid-range",
                    format!("web.{key}: \"{}\" {}", text.trim(), e.describe()),
                )
                .at(at)
                .help(
                    "write an address range like 173.245.48.0/20, or one address like 10.0.0.5/32",
                ),
            ),
            Ok(c) if c.prefix() == 0 => out.push(
                Diagnostic::warning("open-range", format!("web.{key} holds {c}: {open}"))
                    .at(at)
                    .help("list only the addresses of the proxies in front of burrow"),
            ),
            Ok(_) => {}
        }
    }
}

fn proxy_note(
    cfg: &ServerConfig,
    ports: (Option<SocketAddr>, Option<SocketAddr>),
    tunnel: Option<&str>,
    spans: &Spans,
) -> Option<Diagnostic> {
    let web = cfg.web.as_ref()?;
    let files: Vec<&[String]> = web
        .certificates
        .iter()
        .map(|c| c.hosts.as_slice())
        .collect();
    let shared: Vec<String> = web
        .letsencrypt
        .iter()
        .flat_map(|l| l.dns.iter().flat_map(|e| e.wildcards.iter()))
        .filter_map(|w| host_name(w, SingleLabel::Allowed).ok())
        .collect();
    let listed = |h: &str| {
        shared
            .iter()
            .any(|w| w.strip_prefix("*.") == Some(h) || burrow_syntax::web::covers(w, h))
            || !matches!(entry_for(files.iter().copied().enumerate(), h), Ok(None))
    };
    let mut hosts: Vec<(String, String, Option<burrow_syntax::web::Challenge>)> = Vec::new();
    for (name, svc) in &cfg.services {
        let Some(site) = svc.web.as_ref().filter(|_| svc.protocol == Protocol::Tcp) else {
            continue;
        };
        if !matches!(site.tls(), None | Some(SiteTls::Mode(TlsMode::Auto))) {
            continue;
        }
        let choice = match site {
            ServiceWeb::Site(s) => super::challenge::choice(s.challenge),
            _ => None,
        };
        for h in site.hosts() {
            if let Ok(h) = host_name(h, SingleLabel::Refused)
                && !listed(&h)
            {
                hosts.push((name.clone(), h, choice));
            }
        }
    }
    if let Some(ws) = cfg.websocket.as_ref().filter(|w| w.tls.is_none())
        && let Some(h) = tunnel.and_then(|h| host_name(h, SingleLabel::Refused).ok())
        && !listed(&h)
        && !super::on_terminated_site(cfg, &h)
    {
        hosts.push((
            "the tunnel".into(),
            h,
            super::challenge::choice(ws.challenge),
        ));
    }
    let names: Vec<AutoName<'_>> = hosts
        .iter()
        .map(|(label, host, choice)| AutoName {
            label,
            host,
            choice: *choice,
        })
        .collect();
    let dns = web.letsencrypt.as_ref().is_some_and(|l| !l.dns.is_empty());
    let text = note_for(&names, super::challenge::ports_of(ports), dns)?;
    Some(
        Diagnostic::warning("proxy-tls-check", text)
            .at(spans.value(&["web", "trusted_proxies"]))
            .help("only a note: a proxy that passes TCP through leaves the TLS check working; any challenge on a site silences it there"),
    )
}
