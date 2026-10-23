use std::net::SocketAddr;
use std::time::Duration;

use super::super::settings::{Health, Service};
use super::ports::{parse_ranged, ranges_conflict, show_addr, split_target};
use super::{
    MAX_TARGET_LEN, SENSITIVE_PORTS, Spans, name_error, parse_host_port, unix, valid_name,
};
use crate::allowlist::{Cidr, Sources};
use crate::diagnostics::Diagnostic;
use burrow_config::{HealthCheck, HealthCheckSetting, Mode, Protocol, ServiceEntry};

pub(super) struct Taken<'a> {
    pub listen: Option<SocketAddr>,
    pub websocket: Option<SocketAddr>,
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
    if svc.bind.is_none() && svc.visitors.is_empty() && !spans.has_value(&path("bind")) {
        let port = burrow_syntax::ports::FIRST_SERVICE_PORT;
        let d = if svc.protocol == Protocol::Udp {
            Diagnostic::error(
                "unreachable-service",
                format!("service \"{name}\" is udp and has no \"bind\", so nobody can reach it"),
            )
            .label("no bind")
            .help(format!(
                "add \"bind: {port}\" to expose it publicly; visitors can only reach tcp services"
            ))
        } else {
            Diagnostic::error(
                "unreachable-service",
                format!("service \"{name}\" has no \"bind\" and no \"visitors\", so nobody can reach it"),
            )
            .label("no bind and no visitors")
            .help(format!("add \"bind: {port}\" to expose it publicly, or \"visitors: [NAME]\" for visitors using burrow visit"))
        };
        out.push(d.at(spans.key(&["services", name])));
    }
    let target = check_target(name, svc, spans, out);
    if let (Some((_, bind_ports)), Some((_, target_ports))) = (bind, &target)
        && bind_ports != *target_ports
    {
        out.push(
            Diagnostic::error(
                "range-mismatch",
                format!(
                    "service \"{name}\" binds {bind_ports} {} but targets {target_ports}",
                    crate::diagnostics::plural(bind_ports.into(), "port")
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
    let bandwidth = match svc.bandwidth.as_deref().map(crate::ratelimit::parse_rate) {
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
    };
    let clients = check_clients(name, svc, spans, out);
    let health = health(svc);
    for field in ServiceEntry::TCP_ONLY {
        let generic = format!("{field} only works for tcp services");
        let (set, code, why) = match *field {
            "visitors" => (
                !svc.visitors.is_empty(),
                "visitors-tcp-only",
                "visitors can only reach tcp services",
            ),
            "proxy_protocol" => (
                svc.proxy_protocol.is_some(),
                "proxy-protocol-tcp-only",
                "proxy_protocol only works for tcp services",
            ),
            "max_connections" => (
                svc.max_connections.is_some(),
                "max-connections-tcp-only",
                "max_connections only works for tcp services; UDP peers are limited to 4096 per port",
            ),
            "mode" => (
                svc.mode == Mode::Balance && clients.len() > 1,
                "balance-tcp-only",
                "mode balance only works for tcp services",
            ),
            "health_check" => (
                health.is_some(),
                "health-check-tcp-only",
                "health_check only works for tcp services",
            ),
            other => (is_set(svc, other), "tcp-only", generic.as_str()),
        };
        if set {
            tcp_only(name, svc, field, code, why, spans, out);
        }
    }
    let (target, ports) = target?;
    if svc.bind.is_some() && bind.is_none() {
        return None;
    }
    Some(Service {
        clients,
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

fn check_clients(
    name: &str,
    svc: &ServiceEntry,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Vec<String> {
    let mut clients: Vec<String> = Vec::new();
    match (&svc.client, svc.clients.is_empty()) {
        (Some(_), false) => out.push(
            Diagnostic::error(
                "client-and-clients",
                format!("service \"{name}\" has both \"client\" and \"clients\""),
            )
            .at(spans.key(&["services", name, "clients"]))
            .label("use one or the other")
            .help("keep \"client: NAME\" for one client, or list them all under \"clients\""),
        ),
        (None, true) if !spans.has(&["services", name, "client"]) => out.push(
            Diagnostic::error(
                "missing-client",
                format!("service \"{name}\" has no \"client\""),
            )
            .at(spans.key(&["services", name]))
            .label("which client reaches the target?")
            .help("add \"client: NAME\" with a client from \"clients:\", or \"clients: [A, B]\" for several"),
        ),
        _ => {}
    }
    let named = svc.client.iter().map(|c| (None, c));
    let listed = svc.clients.iter().enumerate().map(|(i, c)| (Some(i), c));
    for (i, c) in named.chain(listed) {
        if clients.contains(c) {
            let at = i.map(|i| i.to_string());
            let mut place = vec!["services", name, "clients"];
            place.extend(at.as_deref());
            out.push(
                Diagnostic::error(
                    "duplicate-client",
                    format!("service \"{name}\" lists client \"{c}\" twice"),
                )
                .at(spans.value(&place))
                .help("list each client once"),
            );
            continue;
        }
        clients.push(c.clone());
    }
    clients
}

fn health(svc: &ServiceEntry) -> Option<Health> {
    let tuned = match &svc.health_check {
        None | Some(HealthCheckSetting::On(false)) => return None,
        Some(HealthCheckSetting::On(true)) => HealthCheck::default(),
        Some(HealthCheckSetting::Tuned(h)) => h.clone(),
    };
    Some(Health {
        interval: Duration::from_secs(tuned.interval_secs.into()),
        timeout: Duration::from_secs(tuned.timeout_secs.into()),
        failures: tuned.failures.max(1),
    })
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
        out.push(
            Diagnostic::error(
                code,
                format!("service \"{name}\" is {}, but {why}", svc.protocol.wire()),
            )
            .at(spans.value(&["services", name, field]))
            .help(format!("remove \"{field}\", or use a tcp service")),
        );
    }
}

fn check_target(
    name: &str,
    svc: &ServiceEntry,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<(String, u16)> {
    let text = svc.target.host_port();
    if let Some(path) = unix::path_of(&text) {
        return check_socket(name, svc, &text, path, spans, out);
    }
    let checked = if text.len() > MAX_TARGET_LEN {
        Err(format!(
            "\"{text}\" is longer than {MAX_TARGET_LEN} characters"
        ))
    } else {
        split_target(&text).and_then(|(base, ports)| parse_host_port(&base).map(|_| (base, ports)))
    };
    match checked {
        Ok(t) => Some(t),
        Err(e) => {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid target for service \"{name}\": {e}"),
                )
                .at(spans.value(&["services", name, "target"]))
                .label("expected host:port or a port")
                .help("use host:port as seen from the client, for example 127.0.0.1:3000, a port alone like 3000, a range like 30000-30010, or unix:/path/to.sock"),
            );
            None
        }
    }
}

fn check_socket(
    name: &str,
    svc: &ServiceEntry,
    text: &str,
    path: &str,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<(String, u16)> {
    let at = spans.value(&["services", name, "target"]);
    if svc.protocol != Protocol::Tcp {
        out.push(
            Diagnostic::error(
                "socket-tcp-only",
                format!("service \"{name}\" is udp, but its target is a Unix socket"),
            )
            .at(at)
            .help("Unix socket targets work for tcp services only"),
        );
        return None;
    }
    match unix::check_path(path) {
        Ok(()) => Some((text.to_string(), 1)),
        Err(e) => {
            out.push(
                Diagnostic::error(
                    "invalid-address",
                    format!("invalid target for service \"{name}\": {e}"),
                )
                .at(at)
                .label("expected unix:/absolute/path")
                .help("use the socket's full path as seen from the client, for example unix:/var/run/postgresql/.s.PGSQL.5432"),
            );
            None
        }
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
                "duplicate-bind",
                format!(
                    "service \"{name}\" binds {} {shown}, already used by service \"{other}\"",
                    protocol.wire()
                ),
            )
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
