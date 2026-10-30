use std::net::SocketAddr;
use std::time::Duration;

use super::super::settings::{Health, HttpCheck, Service};
use super::ports::{parse_ranged, ranges_conflict, show_addr};
use super::{SENSITIVE_PORTS, Spans, name_error, valid_name};
use crate::config::diagnostics::Diagnostic;
use crate::policy::allowlist::{Cidr, Sources};
use burrow_config::{HealthCheck, HealthCheckSetting, Mode, Protocol, ServiceEntry};

pub(super) struct Taken<'a> {
    pub listen: Option<SocketAddr>,
    pub websocket: Option<SocketAddr>,
    pub web: Option<SocketAddr>,
    pub web_http: Option<SocketAddr>,
    pub binds: Vec<(SocketAddr, u16, Protocol, &'a str)>,
}

pub(super) fn check_service<'a>(
    name: &'a str,
    svc: &ServiceEntry,
    taken: &mut Taken<'a>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<Service> {
    let path = |f: &'static str| ["services", name, f];
    if !valid_name(name) {
        out.push(name_error("service", name, spans.key(&["services", name])));
    }
    let bind = match svc
        .bind
        .as_ref()
        .map(|b| parse_ranged(b, [0, 0, 0, 0], "bind"))
    {
        None => None,
        Some(Ok(b)) => Some(b),
        Some(Err(e)) => {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid bind address for service \"{name}\": {e}"),
                )
                .at(spans.value(&path("bind")))
                .label("expected a port, a range or ip:port")
                .help(format!(
                    "use a port like {p}, a range like 30000-30010, or ip:port like 127.0.0.1:{p}",
                    p = burrow_syntax::ports::FIRST_SERVICE_PORT
                )),
            );
            None
        }
    };
    check_reachable(name, svc, spans, out);
    let target = super::entries::check_target(
        &super::entries::Whose {
            name,
            client: None,
            protocol: svc.protocol,
            at: spans.value(&path("target")),
        },
        &svc.target,
        out,
    );
    if let (Some((_, bind_ports)), Some((_, target_ports))) = (bind, &target)
        && bind_ports != *target_ports
    {
        out.push(
            Diagnostic::error(
                "range-mismatch",
                format!(
                    "service \"{name}\" binds {bind_ports} {} but targets {target_ports}",
                    crate::config::diagnostics::plural(bind_ports.into(), "port")
                ),
            )
            .at(spans.value(&path("target")))
            .label("not the same number of ports as bind")
            .help("give bind and target ranges of the same size, for example bind: 30000-30010 and target: 30000-30010"),
        );
    }
    if let Some((addr, ports)) = bind {
        check_bind(name, svc.protocol, addr, ports, taken, spans, out);
    }
    let sources = Sources {
        allow: parse_sources(name, "allow_from", &svc.allow_from, spans, out),
        deny: parse_sources(name, "deny_from", &svc.deny_from, spans, out),
    };
    let listed = listed_visitors(name, svc, spans, out);
    let bandwidth = bandwidth(name, svc, spans, out);
    let listed_clients = super::entries::check_clients(name, svc, target.as_ref(), spans, out);
    let health = health(svc);
    check_tcp_only(
        name,
        svc,
        listed_clients.entries.len(),
        health.is_some(),
        spans,
        out,
    );
    let (target, ports) = target?;
    if svc.bind.is_some() && bind.is_none() {
        return None;
    }
    Some(Service {
        clients: listed_clients.names,
        entries: listed_clients.entries,
        mode: svc.mode,
        health,
        bind: bind.map(|(a, _)| a),
        target,
        ports,
        protocol: svc.protocol,
        max_connections: svc.max_connections.map(|m| m as usize),
        visitors: listed,
        sources,
        proxy_protocol: svc.proxy_protocol,
        bandwidth,
    })
}

fn check_reachable(name: &str, svc: &ServiceEntry, spans: &Spans, out: &mut Vec<Diagnostic>) {
    let path = |f: &'static str| ["services", name, f];
    if svc.bind.is_none()
        && svc.visitors.is_empty()
        && svc.web.is_none()
        && !spans.has_value(&path("bind"))
        && !spans.has_value(&path("web"))
    {
        out.push(
            Diagnostic::error(
                burrow_admin_proto::codes::UNREACHABLE_SERVICE,
                format!("service \"{name}\" has no \"bind\" and no \"visitors\", so nobody can reach it"),
            )
            .at(spans.key(&["services", name]))
            .label("no bind and no visitors")
            .help(format!("add \"bind: {}\" to expose it publicly, or \"visitors: [NAME]\" for visitors using burrow visit", burrow_syntax::ports::FIRST_SERVICE_PORT)),
        );
    }
}

fn listed_visitors(
    name: &str,
    svc: &ServiceEntry,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Vec<String> {
    let mut listed = Vec::new();
    for (i, v) in svc.visitors.iter().enumerate() {
        if listed.contains(v) {
            let at = i.to_string();
            out.push(
                Diagnostic::error(
                    "duplicate-visitor",
                    format!("service \"{name}\" lists visitor \"{v}\" twice"),
                )
                .at(spans.value(&["services", name, "visitors", &at]))
                .help("list each visitor once"),
            );
            continue;
        }
        listed.push(v.clone());
    }
    listed
}

fn bandwidth(
    name: &str,
    svc: &ServiceEntry,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<u64> {
    let path = |f: &'static str| ["services", name, f];
    match svc
        .bandwidth
        .as_deref()
        .map(crate::policy::ratelimit::parse_rate)
    {
        None => None,
        Some(Ok(rate)) => Some(rate),
        Some(Err(e)) => {
            out.push(
                Diagnostic::error(
                    "invalid-bandwidth",
                    format!("invalid bandwidth for service \"{name}\": {e}"),
                )
                .at(spans.value(&path("bandwidth")))
                .label("expected a rate like 40mbit")
                .help("use a number with kbit, mbit or gbit (bits per second), or kb, mb or gb (bytes per second), for example 40mbit"),
            );
            None
        }
    }
}

fn check_tcp_only(
    name: &str,
    svc: &ServiceEntry,
    clients: usize,
    health: bool,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let mode_tcp_only = format!("mode {} only works for tcp services", svc.mode.wire());
    for field in ServiceEntry::TCP_ONLY {
        let generic = format!("{field} only works for tcp services");
        let (set, code, why) = match *field {
            "proxy_protocol" => (
                svc.proxy_protocol.is_some(),
                burrow_admin_proto::codes::PROXY_PROTOCOL_TCP_ONLY,
                "proxy_protocol only works for tcp services",
            ),
            "max_connections" => (
                svc.max_connections.is_some(),
                burrow_admin_proto::codes::MAX_CONNECTIONS_TCP_ONLY,
                "max_connections only works for tcp services; for UDP, udp.max_peers limits the peers of each port",
            ),
            "mode" => (
                svc.mode != Mode::Failover && clients > 1,
                burrow_admin_proto::codes::BALANCE_TCP_ONLY,
                mode_tcp_only.as_str(),
            ),
            "health_check" => (
                health,
                burrow_admin_proto::codes::HEALTH_CHECK_TCP_ONLY,
                "health_check only works for tcp services",
            ),
            "web" => (
                is_set(svc, "web"),
                burrow_admin_proto::codes::WEB_TCP_ONLY,
                "web only works for tcp services; remove web:, or set protocol: tcp",
            ),
            other => (is_set(svc, other), "tcp-only", generic.as_str()),
        };
        if set {
            tcp_only(name, svc, field, code, why, spans, out);
        }
    }
}

fn health(svc: &ServiceEntry) -> Option<Health> {
    let tuned = match &svc.health_check {
        None | Some(HealthCheckSetting::On(false)) => return None,
        Some(HealthCheckSetting::On(true)) => HealthCheck::default(),
        Some(HealthCheckSetting::Tuned(h)) => h.clone(),
    };
    let http = tuned
        .path
        .as_deref()
        .zip(check_host(svc))
        .map(|(path, host)| HttpCheck {
            path: burrow_syntax::url_path::route_path(path)
                .unwrap_or_else(|_| path.trim().to_string()),
            host,
        });
    Some(Health {
        interval: Duration::from_secs(tuned.interval_secs.into()),
        timeout: Duration::from_secs(tuned.timeout_secs.into()),
        failures: tuned.failures.max(1),
        http,
    })
}

fn check_host(svc: &ServiceEntry) -> Option<String> {
    use burrow_syntax::web::{SingleLabel, host_name, is_wildcard};
    let hosts: Vec<String> = svc
        .web
        .as_ref()?
        .hosts()
        .iter()
        .filter_map(|h| host_name(h, SingleLabel::Allowed).ok())
        .collect();
    hosts
        .iter()
        .find(|h| !is_wildcard(h))
        .or(hosts.first())
        .map(|h| h.strip_prefix("*.").unwrap_or(h).to_string())
}

fn is_set(svc: &ServiceEntry, field: &str) -> bool {
    serde_json::to_value(svc)
        .ok()
        .and_then(|v| v.get(field).cloned())
        .is_some_and(|v| match v {
            serde_json::Value::Null | serde_json::Value::Bool(false) => false,
            serde_json::Value::Array(a) => !a.is_empty(),
            serde_json::Value::Object(o) => !o.is_empty(),
            _ => true,
        })
}

fn tcp_only(
    name: &str,
    svc: &ServiceEntry,
    field: &'static str,
    code: &'static str,
    why: &str,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    if svc.protocol != Protocol::Tcp {
        let d = Diagnostic::error(
            code,
            format!("service \"{name}\" is {}, but {why}", svc.protocol.wire()),
        )
        .at(spans.value(&["services", name, field]));
        out.push(if code == burrow_admin_proto::codes::WEB_TCP_ONLY {
            d
        } else {
            d.help(format!("remove {field}:, or set protocol: tcp"))
        });
    }
}

fn check_bind<'a>(
    name: &'a str,
    protocol: Protocol,
    bind: SocketAddr,
    ports: u16,
    taken: &mut Taken<'a>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let at = spans.value(&["services", name, "bind"]);
    let shown = show_addr(bind, ports);
    if let Some((_, _, _, other)) = taken
        .binds
        .iter()
        .find(|(b, n, p, _)| *p == protocol && ranges_conflict(*b, *n, bind, ports))
    {
        out.push(
            Diagnostic::error(
                burrow_admin_proto::codes::DUPLICATE_BIND,
                format!(
                    "service \"{name}\" binds {} {shown}, already used by service \"{other}\"",
                    protocol.wire()
                ),
            )
            .about(name)
            .about(*other)
            .at(at)
            .label("address already used")
            .help("pick a different port for one of the services"),
        );
    }
    for (listener, what, field) in [
        (
            taken.websocket,
            "the websocket listener",
            "same as \"websocket.listen\"",
        ),
        (taken.listen, "the control port", "same as \"listen\""),
        (
            taken.web,
            "the web proxy's HTTPS port",
            "same as \"web.https\"",
        ),
        (
            taken.web_http,
            "the web proxy's HTTP port",
            "same as \"web.http\"",
        ),
    ] {
        if let Some(l) = listener
            && protocol == Protocol::Tcp
            && ranges_conflict(l, 1, bind, ports)
        {
            out.push(
                Diagnostic::error(
                    "bind-is-control-port",
                    format!(
                        "service \"{name}\" binds {shown}, which {} {what}",
                        if ports > 1 { "includes" } else { "is" }
                    ),
                )
                .at(at)
                .label(field)
                .help(format!("pick a port other than {}", l.port())),
            );
        }
    }
    if bind.ip().is_unspecified()
        && let Some((port, what)) = SENSITIVE_PORTS
            .iter()
            .find(|(p, _)| (bind.port()..=bind.port().saturating_add(ports - 1)).contains(p))
    {
        out.push(
            Diagnostic::warning(
                "sensitive-port",
                format!("service \"{name}\" exposes port {port} ({what}) on all interfaces"),
            )
            .at(at)
            .label("reachable from the internet")
            .help(format!(
                "bind to 127.0.0.1:{} unless it must be public",
                crate::config::check::ports::show(bind.port(), ports)
            )),
        );
    }
    taken.binds.push((bind, ports, protocol, name));
}

fn parse_sources(
    service: &str,
    field: &'static str,
    list: &[String],
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Vec<Cidr> {
    let mut nets = Vec::new();
    for (i, text) in list.iter().enumerate() {
        match text.parse::<Cidr>() {
            Ok(net) => nets.push(net),
            Err(e) => out.push(
                Diagnostic::error(
                    "invalid-source",
                    format!(
                        "invalid {field} entry for service \"{service}\": {}",
                        e.describe()
                    ),
                )
                .at(spans.value(&["services", service, field, &i.to_string()]))
                .label("expected an IP or CIDR")
                .help(
                    "use an IP like 203.0.113.7 or a network like 203.0.113.0/24 or 2001:db8::/32",
                ),
            ),
        }
    }
    nets
}
