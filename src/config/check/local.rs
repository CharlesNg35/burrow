use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::super::settings::Service;
use super::{Spans, parse_host_port, unix};
use crate::allowlist::{AllowList, Gate, NetRule, Refusal, Rule};
use crate::config::diagnostics::Diagnostic;
use burrow_config::SELF;
use burrow_config::{Protocol, ServerConfig};

pub(super) fn check_allow(
    cfg: &ServerConfig,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<AllowList> {
    let entry = cfg.self_.as_ref()?;
    let rules = entry
        .allow
        .iter()
        .enumerate()
        .filter_map(|(i, text)| {
            let at = spans.value(&["self", "allow", &i.to_string()]);
            let rule = super::client::parse_rule(text, "self.allow", at, out)?;
            if let Rule::Net(net) = &rule
                && net.net.prefix() == 0
            {
                out.push(whole_family(net, at));
            }
            Some(rule)
        })
        .collect();
    Some(AllowList::Rules(rules))
}

fn whole_family(net: &NetRule, at: crate::config::tree::Span) -> Diagnostic {
    let family = match net.net.addr() {
        IpAddr::V4(_) => "IPv4",
        IpAddr::V6(_) => "IPv6",
    };
    Diagnostic::warning(
        "allow-every-address",
        format!(
            "self.allow {net} lets this server dial any {family} address for services with client: self and reach: self, the internet included"
        ),
    )
    .at(at)
    .label("no restriction on targets")
    .help("list only the networks this server should reach for itself, for example allow: [127.0.0.1/32, 10.0.0.0/24]")
}

pub(super) fn check_reserved(cfg: &ServerConfig, spans: &Spans, out: &mut Vec<Diagnostic>) {
    if cfg.clients.contains_key(SELF) {
        out.push(
            Diagnostic::error(
                "reserved-name",
                format!("a client cannot be named \"{SELF}\""),
            )
            .at(spans.key(&["clients", SELF]))
            .label("reserved for this server")
            .help(format!(
                "\"{SELF}\" means the server itself, in \"client\", \"clients\" and \"reach\"; give the client another name"
            )),
        );
    }
}

pub(super) fn check_reach(
    visitor: &str,
    allow: Option<&AllowList>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    if allow.is_some() {
        return;
    }
    out.push(
        Diagnostic::error(
            "reach-needs-self-allow",
            format!("visitor \"{visitor}\" has reach to \"{SELF}\", but the server config has no self.allow"),
        )
        .at(spans.key(&["visitors", visitor, "reach", SELF]))
        .label("this server would have no limit")
        .help("add what this server may reach for visitors, for example:\n         self:\n           allow: [127.0.0.1/32:22]"),
    );
}

pub(super) fn allow_place(client: &str) -> String {
    if client == SELF {
        "\"self.allow\"".to_string()
    } else {
        format!("\"allow\" on client \"{client}\"")
    }
}

pub(super) fn check_services(
    services: &BTreeMap<String, Service>,
    allow: Option<&AllowList>,
    own: &[(SocketAddr, &str)],
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    for (name, svc) in services
        .iter()
        .filter(|(_, s)| s.clients.iter().any(|c| c == SELF))
    {
        let at = spans.value(&["services", name, "target"]);
        if let Some(path) = unix::path_of(&svc.target) {
            if allow.is_some_and(|a| !a.permits_socket(path)) {
                out.push(outside(name, &svc.target, &svc.target, at));
            }
            continue;
        }
        let mut mine: Vec<(Protocol, SocketAddr, &str)> = own
            .iter()
            .map(|(a, what)| (Protocol::Tcp, *a, *what))
            .collect();
        mine.extend(
            svc.binds()
                .into_iter()
                .map(|(_, a)| (svc.protocol, a, "bind of this service")),
        );
        let gate = Gate::this_server(allow, mine.iter().map(|(p, a, _)| (*p, *a)).collect());
        let what = |addr: SocketAddr| {
            mine.iter()
                .find(|(p, a, _)| *p == svc.protocol && *a == addr)
                .map_or("address", |(_, _, what)| *what)
        };
        let refused = (0..svc.ports.max(1))
            .find_map(|i| refusal_of(&gate, svc.protocol, &literal_addrs(&svc.target_at(i))));
        match refused {
            Some((addr, Refusal::Burrow(own))) => out.push(
                Diagnostic::error(
                    "target-is-burrow",
                    format!(
                        "service \"{name}\" on \"{SELF}\" targets {addr}, which is burrow's own {} ({own})",
                        what(own)
                    ),
                )
                .at(at)
                .label("a loop back into burrow")
                .help("point the service at the application behind it, on another port"),
            ),
            Some((addr, Refusal::LinkLocal)) => out.push(
                Diagnostic::error(
                    "link-local-target",
                    format!("service \"{name}\" on \"{SELF}\" targets {addr}, a link-local or metadata address"),
                )
                .at(at)
                .label("not named in self.allow")
                .help(format!(
                    "list the address itself in self.allow, for example {}; a range that covers it does not count",
                    AllowList::suggestion(addr)
                )),
            ),
            Some((addr, Refusal::Outside)) => out.push(outside(
                name,
                &addr.to_string(),
                &AllowList::suggestion(addr),
                at,
            )),
            None => {}
        }
    }
}

fn outside(name: &str, target: &str, rule: &str, at: crate::config::tree::Span) -> Diagnostic {
    Diagnostic::error(
        "target-outside-self-allow",
        format!(
            "service \"{name}\" on \"{SELF}\" targets {target}, which self.allow does not permit"
        ),
    )
    .at(at)
    .label("not in self.allow")
    .help(format!(
        "add \"{rule}\" to self.allow, or a rule that covers it"
    ))
}

fn refusal_of(
    gate: &Gate,
    protocol: Protocol,
    addrs: &[SocketAddr],
) -> Option<(SocketAddr, Refusal)> {
    let refusals: Vec<(SocketAddr, Option<Refusal>)> = addrs
        .iter()
        .map(|a| (*a, gate.refusal(protocol, *a)))
        .collect();
    let hard = refusals
        .iter()
        .find_map(|(a, r)| r.filter(|r| *r != Refusal::Outside).map(|r| (*a, r)));
    let none_permitted = refusals.iter().all(|(_, r)| r.is_some());
    hard.or_else(|| {
        none_permitted
            .then(|| refusals.first().map(|(a, _)| (*a, Refusal::Outside)))
            .flatten()
    })
}

fn literal_addrs(target: &str) -> Vec<SocketAddr> {
    let Ok((host, port)) = parse_host_port(target) else {
        return Vec::new();
    };
    if host.eq_ignore_ascii_case("localhost") {
        return vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
        ];
    }
    host.parse::<IpAddr>()
        .map(|ip| vec![SocketAddr::new(ip, port)])
        .unwrap_or_default()
}
