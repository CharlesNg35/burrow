use std::collections::BTreeMap;

use super::Served;
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::Shares;
use crate::config::tree::Span;
use burrow_syntax::address::is_localhost;
use burrow_syntax::web::{SingleLabel, host_name};

pub(super) struct Place<'a> {
    pub name: &'a str,
    pub at: &'a dyn Fn(usize) -> Span,
    pub served: Served,
    pub tls: String,
    pub tls_written: Option<String>,
    pub tls_unknown: bool,
    pub client_ca: Option<String>,
    pub paths: Vec<String>,
}

pub(super) struct Owner {
    pub service: String,
    pub served: Served,
    pub tls: String,
    pub tls_written: Option<String>,
    pub client_ca: Option<String>,
    pub paths: Vec<String>,
}

pub(super) type Owners = BTreeMap<String, Vec<Owner>>;

fn clash(place: &Place<'_>, host: &str, owners: &Owners) -> Option<Diagnostic> {
    let name = place.name;
    for o in owners.get(host).into_iter().flatten() {
        let other = &o.service;
        let pass = match (o.served, place.served) {
            (Served::Passthrough, Served::Passthrough) => None,
            (Served::Passthrough, _) => Some(other.as_str()),
            (_, Served::Passthrough) => Some(name),
            _ => None,
        };
        let message = if let Some(pass) = pass {
            format!("host \"{host}\" is used by a passthrough service \"{pass}\" and by a website")
        } else if o.served == Served::Passthrough {
            format!(
                "host \"{host}\" is used by services \"{other}\" and \"{name}\"; each host name leads to one service"
            )
        } else if o.tls != place.tls {
            return Some(tls_clash(o, place, host));
        } else if o.client_ca != place.client_ca {
            format!(
                "host \"{host}\": services share it but name different client_auth CAs; device certificates are checked per host name"
            )
        } else if o.paths.iter().any(|p| place.paths.contains(p)) {
            format!(
                "host \"{host}\" is used by services \"{other}\" and \"{name}\"; give one of them other paths, like paths: [/api]"
            )
        } else {
            continue;
        };
        return Some(Diagnostic::error("duplicate-host", message).help(own_name_help(name, host)));
    }
    None
}

fn tls_shown(key: &str, written: Option<&str>) -> String {
    match written {
        Some(w) => format!("tls: {w}"),
        None => format!("no tls, so {key}"),
    }
}

fn tls_clash(o: &Owner, place: &Place<'_>, host: &str) -> Diagnostic {
    let (other, name) = (&o.service, place.name);
    let help = match (&o.tls_written, &place.tls_written) {
        (Some(w), _) => format!("for example write tls: {w} on service \"{name}\" too"),
        (None, Some(w)) => format!("for example write tls: {w} on service \"{other}\" too"),
        (None, None) => format!("for example remove tls from service \"{name}\""),
    };
    Diagnostic::error(
        "duplicate-host",
        format!(
            "host \"{host}\": services \"{other}\" ({}) and \"{name}\" ({}) share it; use the same tls on every service of this name",
            tls_shown(&o.tls, o.tls_written.as_deref()),
            tls_shown(&place.tls, place.tls_written.as_deref()),
        ),
    )
    .help(help)
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
        Served::Terminated | Served::Plain
            if path == "/"
                || place.paths.iter().any(|p| {
                    let key = burrow_syntax::url_path::route_key(path);
                    burrow_syntax::url_path::on_prefix(key.as_deref().unwrap_or(path), p)
                }) =>
        {
            Some(
            Diagnostic::error(
                "tunnel-path",
                format!(
                    "websocket.path \"{path}\" on \"{host}\" is also served by service \"{}\"; give the tunnel another path",
                    place.name
                ),
            )
            .help("for example path: /burrow under websocket"),
            )
        }
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

pub(super) fn check_hosts(
    place: &Place<'_>,
    given: &[String],
    owners: &mut Owners,
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
        let localhost = place.tls
            == burrow_config::admin_schema::AdminEnum::wire(&burrow_config::TlsMode::Internal)
            && is_localhost(text);
        let single = if matches!(place.served, Served::Plain) || localhost || place.tls_unknown {
            SingleLabel::Allowed
        } else {
            SingleLabel::Refused
        };
        let host = match host_name(text, single) {
            Ok(h) => h,
            Err(_) if host_name(text, SingleLabel::Allowed).is_ok() => {
                let short = text.trim().to_lowercase();
                let like = if place.tls
                    == burrow_config::admin_schema::AdminEnum::wire(
                        &burrow_config::TlsMode::Internal,
                    ) {
                    format!("{short}.lan or {short}.home.arpa")
                } else {
                    format!("{short}.home.example.com")
                };
                out.push(
                    Diagnostic::error(
                        "single-label-host",
                        format!(
                            "services.{name}.web.hosts: \"{}\" is a single-label name; HTTPS finds sites by their full name, so use a full name like {like}, or set tls: off to serve it as plain HTTP",
                            text.trim(),
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
        if let Some(d) = clash(place, &host, owners) {
            out.push(d.at(at));
            continue;
        }
        owners.entry(host.clone()).or_default().push(Owner {
            service: name.to_string(),
            served: place.served,
            tls: place.tls.clone(),
            tls_written: place.tls_written.clone(),
            client_ca: place.client_ca.clone(),
            paths: place.paths.clone(),
        });
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

pub(super) fn tls_key(web: &burrow_config::ServiceWeb, https: bool) -> String {
    use burrow_config::{SiteTls, TlsMode};
    match web.tls() {
        Some(SiteTls::Files(f)) => {
            let text = |v: &Option<String>| v.as_deref().unwrap_or_default().trim().to_string();
            format!("files {} {}", text(&f.cert), text(&f.key))
        }
        Some(SiteTls::Mode(mode)) => mode.wire().into(),
        None if https => TlsMode::Auto.wire().into(),
        None => TlsMode::Off.wire().into(),
    }
}

pub(super) fn tls_written(web: &burrow_config::ServiceWeb) -> Option<String> {
    use burrow_config::SiteTls;
    match web.tls()? {
        SiteTls::Files(f) => {
            let text = |v: &Option<String>| v.as_deref().unwrap_or_default().trim().to_string();
            Some(format!(
                "{{cert: {}, key: {}}}",
                text(&f.cert),
                text(&f.key)
            ))
        }
        SiteTls::Mode(mode) => Some(mode.wire().into()),
    }
}

pub(super) fn tls_unknown(
    spans: &super::Spans,
    name: &str,
    web: &burrow_config::ServiceWeb,
) -> bool {
    web.tls().is_none() && spans.has(&["services", name, "web", "tls"])
}

pub(super) fn shares_with_other_tls(
    cfg: &burrow_config::ServerConfig,
    service: &str,
    host: &str,
    https: bool,
) -> bool {
    use burrow_config::{SiteTls, TlsMode};
    let passthrough = |w: &burrow_config::ServiceWeb| {
        matches!(w.tls(), Some(SiteTls::Mode(TlsMode::Passthrough)))
    };
    let Some(mine) = cfg.services.get(service).and_then(|s| s.web.as_ref()) else {
        return false;
    };
    let names = |w: &burrow_config::ServiceWeb, host: &str| {
        w.hosts()
            .iter()
            .any(|h| host_name(h, SingleLabel::Allowed).is_ok_and(|h| h == host))
    };
    !passthrough(mine)
        && cfg.services.iter().any(|(other, svc)| {
            other != service
                && svc.protocol == burrow_config::Protocol::Tcp
                && svc.web.as_ref().is_some_and(|w| {
                    !passthrough(w) && names(w, host) && tls_key(w, https) != tls_key(mine, https)
                })
        })
}

pub(super) fn paths_of(web: &burrow_config::ServiceWeb) -> Vec<String> {
    let given = match web {
        burrow_config::ServiceWeb::Site(s) => s.paths.as_slice(),
        _ => &[],
    };
    let mut out: Vec<String> = given
        .iter()
        .filter_map(|p| burrow_syntax::url_path::route_key(p).ok())
        .collect();
    if given.is_empty() {
        out.push("/".into());
    }
    out
}

pub(super) fn check_redirect_hosts(
    cfg: &burrow_config::ServerConfig,
    owners: &Owners,
    spans: &super::Spans,
    out: &mut Vec<Diagnostic>,
) {
    let mut from: BTreeMap<String, String> = BTreeMap::new();
    for (name, svc) in &cfg.services {
        let Some(burrow_config::ServiceWeb::Site(site)) = svc.web.as_ref() else {
            continue;
        };
        for (i, text) in site.redirect_from.iter().enumerate() {
            let Ok(host) = host_name(text, SingleLabel::Allowed) else {
                continue;
            };
            let other = owners
                .get(&host)
                .and_then(|o| o.iter().find(|o| o.service != *name))
                .map(|o| format!("is also a host of service \"{}\"", o.service))
                .or_else(|| {
                    from.get(&host)
                        .filter(|o| *o != name)
                        .map(|o| format!("is also in redirect_from of service \"{o}\""))
                });
            if let Some(why) = other {
                out.push(
                    Diagnostic::error(
                        "duplicate-host",
                        format!("services.{name}.web.redirect_from: \"{host}\" {why}; a name either serves one site or redirects to it"),
                    )
                    .at(spans.value(&["services", name, "web", "redirect_from", &i.to_string()]))
                    .help("remove it from one of them"),
                );
            }
            from.entry(host).or_insert_with(|| name.clone());
        }
    }
    for key in cfg.web.iter().flat_map(|w| w.redirect_names.keys()) {
        let text = key.trim();
        let (host, path) = match text.find('/') {
            Some(i) => (&text[..i], &text[i..]),
            None => (text, "/"),
        };
        let (Ok(host), Ok(path)) = (
            host_name(host, SingleLabel::Allowed),
            burrow_syntax::url_path::route_key(path),
        ) else {
            continue;
        };
        let served = owners.get(&host).into_iter().flatten().find_map(|o| {
            if o.served == Served::Passthrough {
                Some(format!(
                    "is the host of the passthrough service \"{}\"; burrow cannot see its requests",
                    o.service
                ))
            } else {
                o.paths
                    .contains(&path)
                    .then(|| format!("is also served by service \"{}\"", o.service))
            }
        });
        let why = served.or_else(|| {
            (path == "/")
                .then(|| from.get(&host))
                .flatten()
                .map(|o| format!("is also in redirect_from of service \"{o}\""))
        });
        if let Some(why) = why {
            out.push(
                Diagnostic::error("redirect-name", format!("web.redirect_names: \"{text}\" {why}"))
                    .at(spans.key(&["web", "redirect_names", key]))
                    .help("a longer path like example.com/shop may redirect while a service keeps the rest of the name"),
            );
        }
    }
}

pub(super) fn client_ca(web: &burrow_config::ServiceWeb) -> Option<String> {
    match web {
        burrow_config::ServiceWeb::Site(s) => s
            .client_auth
            .as_ref()
            .map(|c| c.ca.as_deref().unwrap_or_default().trim().to_string()),
        _ => None,
    }
}
