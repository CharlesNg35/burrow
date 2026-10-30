use super::ports::split_target;
use super::{MAX_TARGET_LEN, Spans, parse_host_port, unix};
use crate::config::diagnostics::{Diagnostic, plural};
use crate::config::settings::Entry;
use crate::config::tree::Span;
use burrow_admin_proto::codes;
use burrow_config::{Mode, Protocol, ServiceClient, ServiceEntry, Target};

pub(super) struct Whose<'a> {
    pub name: &'a str,
    pub client: Option<&'a str>,
    pub protocol: Protocol,
    pub at: Span,
}

impl Whose<'_> {
    fn of(&self) -> String {
        match self.client {
            Some(c) => format!("client \"{c}\" of service \"{}\"", self.name),
            None => format!("service \"{}\"", self.name),
        }
    }
}

pub(super) fn check_target(
    w: &Whose,
    target: &Target,
    out: &mut Vec<Diagnostic>,
) -> Option<(String, u16)> {
    let text = target.host_port();
    if let Some(path) = unix::path_of(&text) {
        return check_socket(w, &text, path, out);
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
                    format!("invalid target for {}: {e}", w.of()),
                )
                .at(w.at)
                .label("expected host:port or a port")
                .help("use host:port as seen from the client, for example 127.0.0.1:3000, a port alone like 3000, a range like 30000-30010, or unix:/path/to.sock"),
            );
            None
        }
    }
}

fn check_socket(
    w: &Whose,
    text: &str,
    path: &str,
    out: &mut Vec<Diagnostic>,
) -> Option<(String, u16)> {
    if w.protocol != Protocol::Tcp {
        let whose = match w.client {
            Some(c) => format!("the target of client \"{c}\""),
            None => "its target".to_string(),
        };
        out.push(
            Diagnostic::error(
                "socket-tcp-only",
                format!(
                    "service \"{}\" is udp, but {whose} is a Unix socket",
                    w.name
                ),
            )
            .at(w.at)
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
                    format!("invalid target for {}: {e}", w.of()),
                )
                .at(w.at)
                .label("expected unix:/absolute/path")
                .help("use the socket's full path as seen from the client, for example unix:/var/run/postgresql/.s.PGSQL.5432"),
            );
            None
        }
    }
}

fn presence(name: &str, svc: &ServiceEntry, spans: &Spans, out: &mut Vec<Diagnostic>) {
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
}

pub(super) struct Listed {
    pub names: Vec<String>,
    pub entries: Vec<Entry>,
}

pub(super) fn check_clients(
    name: &str,
    svc: &ServiceEntry,
    service: Option<&(String, u16)>,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Listed {
    presence(name, svc, spans, out);
    let mut listed = Listed {
        names: Vec::new(),
        entries: Vec::new(),
    };
    let mut seen: Vec<(String, Option<String>)> = Vec::new();
    let named = svc.client.iter().map(|c| (None, c.as_str(), None, None, 1));
    let all = svc
        .clients
        .iter()
        .enumerate()
        .map(|(i, c)| (Some(i), c.name(), c.target(), c.given_weight(), c.weight()));
    for (i, client, own, given, weight) in named.chain(all) {
        let at = i.map(|i| i.to_string());
        let mut place = vec!["services", name, "clients"];
        place.extend(at.as_deref());
        let target =
            own.and_then(|t| entry_target(name, client, t, svc, service, &place, spans, out));
        if own.is_some() && target.is_none() {
            continue;
        }
        let effective = target.clone().or_else(|| service.map(|s| s.0.clone()));
        if seen.iter().any(|(c, t)| c == client && *t == effective) {
            out.push(duplicate(name, client, own.is_some(), spans.value(&place)));
            continue;
        }
        seen.push((client.to_string(), effective));
        if given.is_some() && svc.mode != Mode::Balance {
            let mut w = place.clone();
            w.push(ServiceClient::WEIGHT.key());
            out.push(
                Diagnostic::error(
                    codes::WEIGHT_NOT_BALANCE,
                    format!("service \"{name}\" gives client \"{client}\" a weight, but weights only apply with mode: balance"),
                )
                .at(spans.value(&w))
                .label("only used by mode: balance")
                .help("remove the weight, or set mode: balance"),
            );
        }
        if !listed.names.iter().any(|n| n == client) {
            listed.names.push(client.to_string());
        }
        let service_target = service.map_or("", |s| s.0.as_str());
        listed
            .entries
            .push(Entry::new(client, target, weight, service_target));
    }
    listed
}

fn duplicate(name: &str, client: &str, own: bool, at: Span) -> Diagnostic {
    if own {
        return Diagnostic::error(
            "duplicate-client",
            format!("service \"{name}\" lists client \"{client}\" twice with the same target"),
        )
        .at(at)
        .help("give each entry of the same client its own target, or list the client once");
    }
    Diagnostic::error(
        "duplicate-client",
        format!("service \"{name}\" lists client \"{client}\" twice"),
    )
    .at(at)
    .help("list each client once, or give each entry its own target")
}

#[allow(clippy::too_many_arguments)]
fn entry_target(
    name: &str,
    client: &str,
    own: &Target,
    svc: &ServiceEntry,
    service: Option<&(String, u16)>,
    place: &[&str],
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<String> {
    let mut path = place.to_vec();
    path.push(ServiceClient::TARGET.key());
    let at = spans.value(&path);
    let w = Whose {
        name,
        client: Some(client),
        protocol: svc.protocol,
        at,
    };
    let (base, ports) = check_target(&w, own, out)?;
    if let Some((_, want)) = service
        && ports != *want
    {
        out.push(
            Diagnostic::error(
                "range-mismatch",
                format!(
                    "client \"{client}\" of service \"{name}\" targets {ports} {}, but the service targets {want}",
                    plural(ports.into(), "port")
                ),
            )
            .at(at)
            .label("not the same number of ports as the service target")
            .help("give it a range of the same size as the service's, for example 30000-30010, or one port"),
        );
        return None;
    }
    Some(base)
}
