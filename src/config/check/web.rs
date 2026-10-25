use std::collections::BTreeMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::path::Path;

use super::{Spans, binds_conflict, parse_addr};
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::{Shares, WebSite, WebSocketSettings};
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
    let mut owners = hosts::Owners::new();
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
        let place = hosts::Place {
            name,
            at: &item,
            served: Served::of(web, https.is_some()),
            tls: hosts::tls_key(web, https.is_some()),
            paths: hosts::paths_of(web),
        };
        if let Some(hosts) = hosts::check_hosts(&place, web.hosts(), &mut owners, tunnel, out) {
            sites.insert(
                name.clone(),
                fill::site(cfg, web, &svc.target, hosts, base_dir),
            );
        }
    }
    hosts::check_redirect_hosts(cfg, &owners, spans, out);
    site_keys::check_site_keys(cfg, https.is_some(), base_dir, spans, out);
    let shared_tunnel = tunnel.filter(|t| t.2 == Shares::Https).map(|t| t.0);
    site_wide::check_site_wide(cfg, (https, http), shared_tunnel, base_dir, spans, out);
    certs::check_sources(cfg, (https, http), spans, out);
    #[cfg(feature = "web")]
    pem::check_files(cfg, base_dir, spans, out);
    #[cfg(feature = "web")]
    letsencrypt::check_letsencrypt(cfg, base_dir, spans, out);
    #[cfg(feature = "web")]
    dns_secrets::check_dns_secrets(cfg, base_dir, spans, out);
    #[cfg(feature = "web")]
    dns_entries::check_dns_entries(cfg, base_dir, spans, out);
    #[cfg(feature = "web")]
    dns_entries::wildcard_hint(cfg, spans, out);
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

mod certs;
mod challenge;
mod choices;
#[cfg(feature = "web")]
mod dns_entries;
#[cfg(feature = "web")]
mod dns_secrets;
mod fill;
mod fill_dns;
mod fill_site;
mod hosts;
#[cfg(feature = "web")]
mod letsencrypt;
mod low;
#[cfg(feature = "web")]
mod pem;
mod share;
mod site_keys;
mod site_wide;

pub(super) use fill::settings;
pub(super) use share::share_tunnel;

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "web"))]
mod tests_certs;

#[cfg(all(test, feature = "web"))]
mod tests_challenge;

#[cfg(all(test, feature = "web"))]
mod tests_dns_entries;

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

#[cfg(all(test, feature = "web"))]
mod tests_site_keys;

#[cfg(all(test, feature = "web"))]
mod tests_site_wide;

#[cfg(all(test, not(feature = "web")))]
mod tests_without_web;
