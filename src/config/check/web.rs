use std::collections::BTreeMap;
use std::net::{Ipv6Addr, SocketAddr};

use super::{Spans, binds_conflict, parse_addr};
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::{WebSettings, WebSite, WebSocketSettings};
use crate::config::tree::Span;
use burrow_config::{Bind, Protocol, ServerConfig, ServiceWeb, SiteTls, TlsMode};
use burrow_syntax::web::{host_name, is_wildcard};

pub(super) const BUILT_WITH_WEB: bool = cfg!(feature = "web");

fn bare_port(b: &Bind) -> Option<u16> {
    match b {
        Bind::Port(p) => Some(*p),
        Bind::Addr(s) => s.trim().parse().ok(),
    }
}

pub(super) fn check_https(
    cfg: &ServerConfig,
    listen: Option<SocketAddr>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<SocketAddr> {
    let web = cfg.web.as_ref()?;
    if !BUILT_WITH_WEB {
        out.push(
            Diagnostic::error(
                "no-web-proxy",
                "this build of burrow has no web proxy, so web: cannot be used",
            )
            .at(spans.key(&["web"]))
            .help("use a release build of burrow, or remove web: and the web: of each service"),
        );
        return None;
    }
    let Some(given) = &web.https else {
        out.push(
            Diagnostic::error(
                "web-port",
                "web needs a port: https for HTTPS sites, like https: 443 (plain HTTP sites on web.http are not in this build yet)",
            )
            .at(spans.key(&["web"]))
            .help("add https: 443 under web"),
        );
        return None;
    };
    let at = spans.value(&["web", "https"]);
    let addr = match parse_addr(given, [0, 0, 0, 0], "web.https") {
        Ok(a) if bare_port(given).is_some() => {
            SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), a.port())
        }
        Ok(a) => a,
        Err(e) => {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid web.https address \"{given}\": {e}"),
                )
                .at(at)
                .label("expected a port or ip:port")
                .help(
                    "use the port for HTTPS, like 443, or ip:port like 0.0.0.0:443 for IPv4 only",
                ),
            );
            return None;
        }
    };
    if let Some(l) = listen.filter(|l| binds_conflict(*l, addr)) {
        out.push(
            Diagnostic::error(
                "duplicate-bind",
                format!("web.https {addr} is also the control port (listen {l})"),
            )
            .at(at)
            .label("already used by \"listen\"")
            .help("use another port for web.https or for listen"),
        );
        return None;
    }
    Some(addr)
}

pub(super) fn share_tunnel(
    cfg: &ServerConfig,
    https: Option<SocketAddr>,
    websocket: Option<&mut WebSocketSettings>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let (true, Some(ws), Some(given)) = (BUILT_WITH_WEB, websocket, cfg.websocket.as_ref()) else {
        return;
    };
    let host_at = spans.value(&["websocket", "host"]);
    let same_port = https.is_some_and(|h| h.port() == ws.listen.port());
    let shared = match https {
        Some(h) if same_port => {
            let wanted = match bare_port(&given.listen) {
                Some(p) => SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), p),
                None => ws.listen,
            };
            if wanted != h {
                out.push(
                    Diagnostic::error(
                        "web-overlap",
                        format!(
                            "websocket.listen {wanted} and web.https {h} overlap; to share the port give both the same address, otherwise use another port"
                        ),
                    )
                    .at(spans.value(&["websocket", "listen"]))
                    .label("same port as web.https")
                    .help(format!(
                        "write websocket.listen the same way as web.https ({}) to share it",
                        cfg.web
                            .as_ref()
                            .and_then(|w| w.https.as_ref())
                            .map_or(String::new(), Bind::to_string)
                    )),
                );
                return;
            }
            ws.listen = h;
            true
        }
        _ => false,
    };
    let Some(text) = given.host.as_deref() else {
        if shared {
            out.push(
                Diagnostic::error(
                    "tunnel-host",
                    format!(
                        "websocket.host is required when websocket.listen is the same port as web.https ({}): it is the name that leads to the tunnel",
                        ws.listen.port()
                    ),
                )
                .at(spans.key(&["websocket"]))
                .help("add host: with the tunnel's name, like host: tunnel.example.com"),
            );
        }
        return;
    };
    if !shared {
        out.push(
            Diagnostic::error(
                "tunnel-host",
                "websocket.host is only used when websocket.listen is the same port as web.https; remove it, or set websocket.listen to the port of web.https",
            )
            .at(host_at)
            .help("for example web: {https: 443} with websocket: {listen: 443, host: tunnel.example.com}"),
        );
        return;
    }
    let host = match host_name(text) {
        Ok(h) if is_wildcard(&h) => {
            out.push(
                Diagnostic::error(
                    "invalid-host",
                    format!("websocket.host \"{text}\" is a wildcard; the tunnel needs one name"),
                )
                .at(host_at)
                .help("write one name, like tunnel.example.com"),
            );
            return;
        }
        Ok(h) => h,
        Err(e) => {
            out.push(
                Diagnostic::error("invalid-host", format!("websocket.host: {e}"))
                    .at(host_at)
                    .help("write one name, like tunnel.example.com"),
            );
            return;
        }
    };
    if ws.tls.is_none() {
        out.push(
            Diagnostic::error(
                "tunnel-certificate",
                format!(
                    "websocket.host \"{host}\" has no certificate; add websocket.tls with its cert and key"
                ),
            )
            .at(host_at)
            .help("for example tls: {cert: certs/fullchain.pem, key: certs/privkey.pem} under websocket"),
        );
    }
    ws.host = Some(host);
}

pub(super) fn check_sites(
    cfg: &ServerConfig,
    https: Option<SocketAddr>,
    tunnel: Option<&str>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> BTreeMap<String, WebSite> {
    let mut sites = BTreeMap::new();
    if !BUILT_WITH_WEB {
        no_web_proxy_for_sites(cfg, spans, out);
        return sites;
    }
    let mut owners: BTreeMap<String, String> = BTreeMap::new();
    for (name, svc) in &cfg.services {
        let Some(web) = &svc.web else {
            continue;
        };
        let at = spans.value(&["services", name, "web"]);
        if cfg.web.is_none() && !spans.has(&["web"]) {
            out.push(
                Diagnostic::error(
                    "web-off",
                    format!(
                        "services.{name}.web: the web proxy is off; add a top-level web: with its port, like web: {{https: 443}}"
                    ),
                )
                .at(at)
                .help("or remove web: from this service"),
            );
            continue;
        }
        if https.is_none() || svc.protocol != Protocol::Tcp {
            continue;
        }
        if let Some(d) = tls_later(name, web, spans) {
            out.push(d);
            continue;
        }
        let item = |i: usize| {
            let i = i.to_string();
            match web {
                ServiceWeb::Host(_) => spans.value(&["services", name, "web"]),
                ServiceWeb::Hosts(_) => spans.value(&["services", name, "web", &i]),
                ServiceWeb::Site(_) => spans.value(&["services", name, "web", "hosts", &i]),
            }
        };
        let place = Place { name, at: &item };
        if let Some(hosts) = check_hosts(&place, web.hosts(), &mut owners, tunnel, out) {
            sites.insert(name.clone(), WebSite { hosts });
        }
    }
    sites
}

fn tls_later(name: &str, web: &ServiceWeb, spans: &Spans) -> Option<Diagnostic> {
    let (code, why, at) = match web.tls() {
        Some(SiteTls::Mode(TlsMode::Passthrough)) => return None,
        Some(SiteTls::Mode(TlsMode::Off)) => (
            "web-http-later",
            "plain HTTP websites need web.http, which this build does not have yet",
            spans.value(&["services", name, "web", "tls"]),
        ),
        Some(_) => (
            "web-https-later",
            "burrow cannot serve HTTPS for websites in this build yet",
            spans.value(&["services", name, "web", "tls"]),
        ),
        None => (
            "web-https-later",
            "burrow cannot serve HTTPS for websites in this build yet",
            spans.value(&["services", name, "web"]),
        ),
    };
    let key = if web.tls().is_some() { ".tls" } else { "" };
    Some(
        Diagnostic::error(
            code,
            format!("services.{name}.web{key}: {why}; use tls: passthrough"),
        )
        .at(at)
        .help(format!(
            "write web: {{hosts: [{}], tls: passthrough}} to pass the TLS through to the app",
            web.hosts().join(", ")
        )),
    )
}

fn no_web_proxy_for_sites(cfg: &ServerConfig, spans: &Spans, out: &mut Vec<Diagnostic>) {
    if cfg.web.is_some() || spans.has(&["web"]) {
        return;
    }
    for (name, _) in cfg.services.iter().filter(|(_, s)| s.web.is_some()) {
        out.push(
            Diagnostic::error(
                "no-web-proxy",
                format!("services.{name}.web: this build of burrow has no web proxy"),
            )
            .at(spans.value(&["services", name, "web"]))
            .help("use a release build of burrow, or remove web: from this service"),
        );
    }
}

struct Place<'a> {
    name: &'a str,
    at: &'a dyn Fn(usize) -> Span,
}

fn check_hosts(
    place: &Place<'_>,
    given: &[String],
    owners: &mut BTreeMap<String, String>,
    tunnel: Option<&str>,
    out: &mut Vec<Diagnostic>,
) -> Option<Vec<String>> {
    let name = place.name;
    if given.is_empty() {
        out.push(
            Diagnostic::error(
                "invalid-host",
                format!("services.{name}.web.hosts: a website needs at least one host name"),
            )
            .at((place.at)(0))
            .help("add a name, like hosts: [nas.example.com]"),
        );
        return None;
    }
    let mut hosts = Vec::new();
    let before = out.len();
    for (i, text) in given.iter().enumerate() {
        let at = (place.at)(i);
        let host = match host_name(text) {
            Ok(h) => h,
            Err(e) => {
                out.push(
                    Diagnostic::error("invalid-host", format!("services.{name}.web.hosts: {e}"))
                        .at(at)
                        .help("write a name like nas.example.com, a wildcard like *.example.com, or an IP address"),
                );
                continue;
            }
        };
        if hosts.contains(&host) {
            out.push(
                Diagnostic::error(
                    "duplicate-host",
                    format!("services.{name}.web.hosts lists \"{host}\" twice"),
                )
                .at(at)
                .help("list each name once"),
            );
            continue;
        }
        if let Some(other) = owners.insert(host.clone(), name.to_string()) {
            out.push(
                Diagnostic::error(
                    "duplicate-host",
                    format!(
                        "host \"{host}\" is used by services \"{other}\" and \"{name}\"; each host name leads to one service"
                    ),
                )
                .at(at)
                .help("remove it from one of them"),
            );
            continue;
        }
        if tunnel == Some(host.as_str()) {
            out.push(
                Diagnostic::error(
                    "tunnel-host",
                    format!(
                        "websocket.host \"{host}\" is a passthrough site; burrow cannot see its requests"
                    ),
                )
                .at(at)
                .help("give the tunnel its own name, like tunnel.example.com"),
            );
            continue;
        }
        hosts.push(host);
    }
    (out.len() == before).then_some(hosts)
}

pub(super) fn settings(
    https: Option<SocketAddr>,
    sites: BTreeMap<String, WebSite>,
    websocket: Option<&WebSocketSettings>,
) -> Option<WebSettings> {
    Some(WebSettings {
        https: https?,
        sites,
        tunnel: websocket.and_then(|w| w.host.clone()),
    })
}

#[cfg(test)]
mod tests;

#[cfg(all(test, not(feature = "web")))]
mod tests_without_web;
