use std::collections::BTreeMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::path::Path;

use super::{Spans, binds_conflict, parse_addr};
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::{Shares, WebSite, WebSocketSettings};
use crate::config::tree::Span;
use burrow_config::{Bind, Protocol, ServerConfig, ServiceWeb, SiteTls, TlsMode};
use burrow_syntax::web::{SingleLabel, host_name};

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
    let Some(given) = cfg.web.as_ref().and_then(|w| w.http.as_ref()) else {
        if https.is_some() && cfg.services.values().any(|s| s.web.is_some()) {
            out.push(
                Diagnostic::warning(
                    "no-http",
                    "web.http is not set: http:// addresses get no answer",
                )
                .at(spans.key(&["web"]))
                .help("add http: 80 so http:// addresses are sent to https://"),
            );
        }
        return None;
    };
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
        && ws.shares != Shares::Http
        && binds_conflict(h, ws.listen)
    {
        out.push(
            Diagnostic::error(
                "duplicate-bind",
                format!("web.http {h} is also websocket.listen {}", ws.listen),
            )
            .at(spans.value(&["web", "http"]))
            .label("already used by websocket.listen")
            .help("to share it, write websocket.listen the same way as web.http and give websocket.host; otherwise use another port for one of them"),
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
    ports: (Option<SocketAddr>, Option<SocketAddr>),
    ws: &mut WebSocketSettings,
    listen: &Bind,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Result<Shares, ()> {
    let wanted = match bare_port(listen) {
        Some(p) => SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), p),
        None => ws.listen,
    };
    let candidates = [
        (ports.0, Shares::Https, "https"),
        (ports.1, Shares::Http, "http"),
    ];
    for (port, shares, key) in candidates {
        let Some(h) = port.filter(|h| h.port() == ws.listen.port()) else {
            continue;
        };
        if wanted == h {
            ws.listen = h;
            return Ok(shares);
        }
        if !binds_conflict(wanted, h) {
            continue;
        }
        let written = cfg
            .web
            .as_ref()
            .and_then(|w| {
                if key == "https" {
                    w.https.as_ref()
                } else {
                    w.http.as_ref()
                }
            })
            .map_or(String::new(), Bind::to_string);
        out.push(
            Diagnostic::error(
                "web-overlap",
                format!(
                    "websocket.listen {wanted} and web.{key} {h} overlap; to share the port give both the same address, otherwise use another port"
                ),
            )
            .at(spans.value(&["websocket", "listen"]))
            .label(format!("same port as web.{key}"))
            .help(format!(
                "write websocket.listen the same way as web.{key} ({written}) to share it"
            )),
        );
        return Err(());
    }
    Ok(Shares::Nothing)
}

fn plain_on_443(cfg: &ServerConfig, ws: &WebSocketSettings, spans: &Spans) -> Option<Diagnostic> {
    if ws.listen.port() != 443 || ws.tls.is_some() {
        return None;
    }
    let host = cfg.public_address.as_deref().map_or("this-server", |a| {
        let a = a.rsplit("://").next().unwrap_or(a);
        let a = a.split('/').next().unwrap_or(a);
        match a.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() && !h.contains(':') => h,
            _ => a,
        }
    });
    Some(
        Diagnostic::warning(
            "plain-ws-443",
            format!(
                "websocket.listen 443 has no TLS: clients must use ws://{host}:443{}, and networks that expect TLS on 443 may block it; add websocket.tls, use web.letsencrypt (a web: block with websocket.host), or use port 80",
                ws.path
            ),
        )
        .at(spans.value(&["websocket", "listen"]))
        .help("the tunnel stays encrypted by burrow either way"),
    )
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
    tunnel: Option<&WebSocketSettings>,
    spans: &Spans,
    base_dir: &Path,
    out: &mut Vec<Diagnostic>,
) -> BTreeMap<String, WebSite> {
    let mut sites = BTreeMap::new();
    if !BUILT_WITH_WEB {
        no_web_proxy_for_sites(cfg, spans, out);
        return sites;
    }
    let tunnel = tunnel.and_then(|w| w.host.as_deref().map(|h| (h, w.path.as_str(), w.shares)));
    let mut owners: BTreeMap<String, (String, Served)> = BTreeMap::new();
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
                        "services.{name}.web: the web proxy is off; add a top-level web: with its ports, like web: {{https: 443, http: 80}}"
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
            sites.insert(
                name.clone(),
                fill::site(cfg, web, &svc.target, hosts, base_dir),
            );
        }
    }
    certs::check_sources(cfg, (https, http), spans, out);
    #[cfg(feature = "web")]
    pem::check_files(cfg, base_dir, spans, out);
    #[cfg(feature = "web")]
    letsencrypt::check_letsencrypt(cfg, base_dir, spans, out);
    #[cfg(feature = "web")]
    dns_secrets::check_dns_secrets(cfg, base_dir, spans, out);
    choices::check_choices(cfg, (https, http), spans, out);
    low::check_low_ports((https, http), spans, out);
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

fn tunnel_on_site(place: &Place<'_>, host: &str, path: &str, shares: Shares) -> Option<Diagnostic> {
    let own = "give the tunnel its own name, like tunnel.example.com";
    match place.served {
        Served::Passthrough => Some(
            Diagnostic::error(
                "tunnel-host",
                format!("websocket.host \"{host}\" is a passthrough site; burrow cannot see its requests"),
            )
            .help(own),
        ),
        Served::Plain if shares == Shares::Https => Some(
            Diagnostic::error(
                "tunnel-host",
                format!("websocket.host \"{host}\" is a plain-HTTP site (tls: off); the tunnel needs HTTPS"),
            )
            .help(own),
        ),
        Served::Terminated | Served::Plain if path == "/" => Some(
            Diagnostic::error(
                "tunnel-path",
                format!(
                    "websocket.path \"{path}\" on \"{host}\" is also served by service \"{}\"; give the tunnel another path",
                    place.name
                ),
            )
            .help("for example path: /burrow under websocket"),
        ),
        Served::Terminated | Served::Plain => None,
    }
}

fn own_name_help(name: &str, host: &str) -> String {
    let parent = host
        .strip_prefix("*.")
        .or_else(|| {
            host.split_once('.')
                .map(|(_, p)| p)
                .filter(|p| p.contains('.'))
        })
        .or(Some(host))
        .filter(|_| !burrow_syntax::web::is_ip(host));
    match parent {
        Some(parent) => format!(
            "remove it from one of them, or give service \"{name}\" a name of its own, like {name}.{parent}"
        ),
        None => "remove it from one of them".into(),
    }
}

fn check_hosts(
    place: &Place<'_>,
    given: &[String],
    owners: &mut BTreeMap<String, (String, Served)>,
    tunnel: Option<(&str, &str, Shares)>,
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
        let single = if matches!(place.served, Served::Plain) {
            SingleLabel::Allowed
        } else {
            SingleLabel::Refused
        };
        let host = match host_name(text, single) {
            Ok(h) => h,
            Err(_) if host_name(text, SingleLabel::Allowed).is_ok() => {
                out.push(
                    Diagnostic::error(
                        "single-label-host",
                        format!(
                            "services.{name}.web.hosts: \"{}\" is a single-label name; HTTPS finds sites by their full name, so use a full name like {}.home.example.com, or set tls: off to serve it as plain HTTP",
                            text.trim(),
                            text.trim().to_lowercase()
                        ),
                    )
                    .at(at)
                    .help(
                        "SNI carries a fully qualified DNS name (RFC 6066 section 3), and public CAs do not issue certificates for single-label names",
                    ),
                );
                continue;
            }
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
        if let Some((other, served)) = owners.insert(host.clone(), (name.to_string(), place.served))
        {
            let message = match (served, place.served) {
                (Served::Passthrough, Served::Passthrough) => None,
                (Served::Passthrough, _) => Some(&other[..]),
                (_, Served::Passthrough) => Some(name),
                _ => None,
            }
            .map_or_else(
                || format!("host \"{host}\" is used by services \"{other}\" and \"{name}\"; each host name leads to one service"),
                |pass| format!("host \"{host}\" is used by a passthrough service \"{pass}\" and by a website"),
            );
            out.push(
                Diagnostic::error("duplicate-host", message)
                    .at(at)
                    .help(own_name_help(name, &host)),
            );
            continue;
        }
        if let Some((_, path, shares)) = tunnel.filter(|(t, _, _)| *t == host)
            && let Some(d) = tunnel_on_site(place, &host, path, shares)
        {
            out.push(d.at(at));
            continue;
        }
        hosts.push(host);
    }
    (out.len() == before).then_some(hosts)
}

mod certs;
mod challenge;
mod choices;
#[cfg(feature = "web")]
mod dns_secrets;
mod fill;
mod fill_dns;
mod fill_site;
#[cfg(feature = "web")]
mod letsencrypt;
mod low;
#[cfg(feature = "web")]
mod pem;
mod share;

pub(super) use fill::settings;
pub(super) use share::share_tunnel;

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "web"))]
mod tests_certs;

#[cfg(all(test, feature = "web"))]
mod tests_challenge;

#[cfg(all(test, feature = "web"))]
mod tests_dns_secrets;

#[cfg(all(test, feature = "web"))]
mod tests_dns_settings;

#[cfg(all(test, feature = "web"))]
mod tests_docs;

#[cfg(all(test, feature = "web"))]
mod tests_files;

#[cfg(all(test, feature = "web"))]
mod tests_fixtures;

#[cfg(all(test, feature = "web"))]
mod tests_keys;

#[cfg(all(test, feature = "web"))]
mod tests_tunnel;

#[cfg(all(test, feature = "web"))]
mod tests_pem;

#[cfg(all(test, feature = "web"))]
mod tests_settings;

#[cfg(all(test, feature = "web"))]
mod tests_sharing;

#[cfg(all(test, feature = "web"))]
mod tests_site_settings;

#[cfg(all(test, not(feature = "web")))]
mod tests_without_web;
