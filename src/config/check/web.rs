use std::collections::BTreeMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::path::Path;

use super::{Spans, binds_conflict, parse_addr};
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::{WebSite, WebSocketSettings};
use crate::config::tree::Span;
use burrow_config::{Bind, Protocol, ServerConfig, ServiceWeb, SiteTls, TlsMode};
use burrow_syntax::web::{SingleLabel, host_name, is_wildcard};

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
    let written = cfg.web.is_some() || spans.is_null(&["web"]);
    if !written {
        return None;
    }
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
    let Some(given) = cfg.web.as_ref().and_then(|w| w.https.as_ref()) else {
        let http =
            cfg.web.as_ref().is_some_and(|w| w.http.is_some()) || spans.has(&["web", "http"]);
        if !http {
            out.push(
                Diagnostic::error(
                    "web-port",
                    "web needs a port: https for HTTPS sites, like https: 443, or http for plain HTTP sites, like http: 80",
                )
                .at(spans.key(&["web"]))
                .help("add https: 443 or http: 80 under web"),
            );
        }
        return None;
    };
    web_port(&Port::HTTPS, given, listen, spans, out)
}

pub(super) fn check_http(
    cfg: &ServerConfig,
    listen: Option<SocketAddr>,
    https: Option<SocketAddr>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<SocketAddr> {
    let given = cfg.web.as_ref().and_then(|w| w.http.as_ref())?;
    let addr = web_port(&Port::HTTP, given, listen, spans, out)?;
    if let Some(h) = https.filter(|h| binds_conflict(*h, addr)) {
        out.push(
            Diagnostic::error(
                "duplicate-bind",
                format!("web.http {addr} is also web.https {h}"),
            )
            .at(spans.value(&["web", "http"]))
            .label("already used by web.https")
            .help("use two ports, like https: 443 and http: 80"),
        );
        return None;
    }
    Some(addr)
}

pub(super) fn http_beside_websocket(
    http: Option<SocketAddr>,
    ws: Option<&WebSocketSettings>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    if let (Some(h), Some(ws)) = (http, ws)
        && binds_conflict(h, ws.listen)
    {
        out.push(
            Diagnostic::error(
                "duplicate-bind",
                format!("web.http {h} is also websocket.listen {}", ws.listen),
            )
            .at(spans.value(&["web", "http"]))
            .label("already used by websocket.listen")
            .help("the tunnel can share only web.https; use another port for one of them"),
        );
    }
}

struct Port {
    key: &'static str,
    example: u16,
}

impl Port {
    const HTTPS: Port = Port {
        key: "https",
        example: 443,
    };
    const HTTP: Port = Port {
        key: "http",
        example: 80,
    };
}

fn web_port(
    port: &Port,
    given: &Bind,
    listen: Option<SocketAddr>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<SocketAddr> {
    let (key, n) = (port.key, port.example);
    let name = format!("web.{key}");
    let at = spans.value(&["web", key]);
    let addr = match parse_addr(given, [0, 0, 0, 0], &name) {
        Ok(a) if bare_port(given).is_some() => {
            SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), a.port())
        }
        Ok(a) => a,
        Err(e) => {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid {name} address \"{given}\": {e}"),
                )
                .at(at)
                .label("expected a port or ip:port")
                .help(format!(
                    "use the port for {}, like {n}, or ip:port like 0.0.0.0:{n} for IPv4 only",
                    key.to_uppercase()
                )),
            );
            return None;
        }
    };
    if let Some(l) = listen.filter(|l| binds_conflict(*l, addr)) {
        out.push(
            Diagnostic::error(
                "duplicate-bind",
                format!("{name} {addr} is also the control port (listen {l})"),
            )
            .at(at)
            .label("already used by \"listen\"")
            .help(format!("use another port for {name} or for listen")),
        );
        return None;
    }
    Some(addr)
}

fn shares_port(
    cfg: &ServerConfig,
    https: Option<SocketAddr>,
    ws: &mut WebSocketSettings,
    listen: &Bind,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Result<bool, ()> {
    let Some(h) = https.filter(|h| h.port() == ws.listen.port()) else {
        return Ok(false);
    };
    let wanted = match bare_port(listen) {
        Some(p) => SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), p),
        None => ws.listen,
    };
    if wanted == h {
        ws.listen = h;
        return Ok(true);
    }
    if !binds_conflict(wanted, h) {
        return Ok(false);
    }
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
    Err(())
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
    let Ok(shared) = shares_port(cfg, https, ws, &given.listen, spans, out) else {
        return;
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
    let host = match host_name(text, SingleLabel::Refused) {
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
    if !on_terminated_site(cfg, &host) {
        certs::tunnel_has_source(cfg, &host, ws.tls.is_some(), spans, out);
    }
    ws.host = Some(host);
}

pub(super) fn on_terminated_site(cfg: &ServerConfig, host: &str) -> bool {
    let https = cfg.web.as_ref().is_some_and(|w| w.https.is_some());
    cfg.services
        .values()
        .filter_map(|s| s.web.as_ref())
        .any(|web| {
            Served::of(web, https) == Served::Terminated
                && web
                    .hosts()
                    .iter()
                    .any(|h| host_name(h, SingleLabel::Refused).is_ok_and(|h| h == host))
        })
}

pub(super) fn check_sites(
    cfg: &ServerConfig,
    (https, http): (Option<SocketAddr>, Option<SocketAddr>),
    tunnel: Option<(&str, &str)>,
    spans: &Spans,
    base_dir: &Path,
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
        if (https.is_none() && http.is_none()) || svc.protocol != Protocol::Tcp {
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
        let place = Place {
            name,
            at: &item,
            served: Served::of(web, https.is_some()),
        };
        if let Some(hosts) = check_hosts(&place, web.hosts(), &mut owners, tunnel, out) {
            sites.insert(name.clone(), fill::site(cfg, web, hosts, base_dir));
        }
    }
    certs::check_sources(cfg, https, spans, out);
    choices::check_choices(cfg, (https, http), spans, out);
    sites
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
    served: Served,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Served {
    Passthrough,
    Plain,
    Terminated,
}

impl Served {
    fn of(web: &ServiceWeb, https: bool) -> Served {
        match web.tls() {
            Some(SiteTls::Mode(TlsMode::Passthrough)) => Served::Passthrough,
            Some(SiteTls::Mode(TlsMode::Off)) => Served::Plain,
            None if !https => Served::Plain,
            _ => Served::Terminated,
        }
    }
}

fn tunnel_on_site(place: &Place<'_>, host: &str, path: &str) -> Option<Diagnostic> {
    let own = "give the tunnel its own name, like tunnel.example.com";
    match place.served {
        Served::Passthrough => Some(
            Diagnostic::error(
                "tunnel-host",
                format!("websocket.host \"{host}\" is a passthrough site; burrow cannot see its requests"),
            )
            .help(own),
        ),
        Served::Plain => Some(
            Diagnostic::error(
                "tunnel-host",
                format!("websocket.host \"{host}\" is a plain-HTTP site (tls: off); the tunnel needs HTTPS"),
            )
            .help(own),
        ),
        Served::Terminated if path == "/" => Some(
            Diagnostic::error(
                "tunnel-path",
                format!(
                    "websocket.path \"{path}\" on \"{host}\" is also served by service \"{}\"; give the tunnel another path",
                    place.name
                ),
            )
            .help("for example path: /burrow under websocket"),
        ),
        Served::Terminated => None,
    }
}

fn check_hosts(
    place: &Place<'_>,
    given: &[String],
    owners: &mut BTreeMap<String, String>,
    tunnel: Option<(&str, &str)>,
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
        let host = match host_name(text, SingleLabel::Refused) {
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
        if let Some((_, path)) = tunnel.filter(|(t, _)| *t == host)
            && let Some(d) = tunnel_on_site(place, &host, path)
        {
            out.push(d.at(at));
            continue;
        }
        hosts.push(host);
    }
    (out.len() == before).then_some(hosts)
}

mod certs;
mod choices;
mod fill;

pub(super) use fill::settings;

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "web"))]
mod tests_certs;

#[cfg(all(test, feature = "web"))]
mod tests_files;

#[cfg(all(test, feature = "web"))]
mod tests_keys;

#[cfg(all(test, feature = "web"))]
mod tests_settings;

#[cfg(all(test, feature = "web"))]
mod tests_tunnel;

#[cfg(all(test, not(feature = "web")))]
mod tests_without_web;
