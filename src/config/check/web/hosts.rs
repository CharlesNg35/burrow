use std::collections::BTreeMap;

use super::Served;
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::Shares;
use crate::config::tree::Span;
use burrow_syntax::web::{SingleLabel, host_name};

pub(super) struct Place<'a> {
    pub name: &'a str,
    pub at: &'a dyn Fn(usize) -> Span,
    pub served: Served,
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

pub(super) fn check_hosts(
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
