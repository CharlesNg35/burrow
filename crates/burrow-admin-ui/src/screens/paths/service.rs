use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{HealthCheck, Protocol, ServiceEntry};
use burrow_ui_kit::{Edge, EdgeState, Hop, HopState, PathData, Tone};

use crate::form::plain;

use super::{Problems, clients_of, online};

pub fn service(
    name: &str,
    svc: &Value,
    status: Option<&Status>,
    problems: &Problems,
    editing: bool,
    reserved: &[String],
) -> PathData {
    let st = status.and_then(|s| s.services.iter().find(|x| x.name == name));
    let open = st.map(|x| x.open).unwrap_or(0);
    let public = burrow_admin_logic::web::is_public(svc);
    let source = source_hop(svc, public, problems);
    let server = server_hop(svc, public, st.is_some());
    let clients = clients_of(svc);
    let local = |c: &String| reserved.contains(c);
    let on_server = !clients.is_empty() && clients.iter().all(local);
    let up = clients
        .iter()
        .filter(|c| local(c) || online(status, c))
        .count();
    let shown: Vec<String> = clients
        .iter()
        .map(|c| match local(c) {
            true => crate::domain::THIS_SERVER.to_string(),
            false => c.clone(),
        })
        .collect();
    let healthy = st.map(|x| x.health.iter().filter(|h| h.up).count());
    let health_on = ServiceEntry::HEALTH_CHECK
        .get(svc)
        .is_some_and(|h| !h.is_null() && h != &Value::Bool(false));
    let ctone = if clients.is_empty() || up == 0 {
        Tone::Danger
    } else if up < clients.len() {
        Tone::Warning
    } else {
        Tone::Success
    };
    let client_hop = Hop {
        icon: "monitor",
        head: if clients.len() > 1 {
            "Clients".into()
        } else {
            "Client".into()
        },
        value: if clients.is_empty() {
            "none".into()
        } else {
            shown.join(" · ")
        },
        dot: Some(ctone),
        unit: match (health_on, healthy) {
            (true, Some(h)) if st.is_some_and(|x| !x.health.is_empty()) => {
                format!("{h} of {} healthy", clients.len())
            }
            _ => format!("{up} of {} online", clients.len()),
        },
        state: if problems.clients.is_some() {
            HopState::Warn
        } else if up == 0 && !editing {
            HopState::Bad
        } else {
            HopState::Normal
        },
        badge: problems.clients.clone().map(|b| (b, Tone::Warning)),
        mono: false,
    };
    let target_hop = target_hop(svc, problems, up, on_server, &clients);
    let mut edges = service_edges(svc, problems, up, health_on, clients.len() > 1);
    let kind = match (bound(svc), public) {
        (true, _) => "Public port",
        (false, true) => "Website",
        (false, false) => "Private service",
    };
    if on_server {
        let mode = edges.remove(1);
        if edges[1].below.is_empty() {
            edges[1].below = mode.below;
        }
        let summary = format!(
            "{kind} {} on the server, to {} on this server; {open} open",
            server.value, target_hop.value
        );
        return PathData {
            hops: vec![source, server, target_hop],
            edges,
            summary,
        };
    }
    let summary = format!(
        "{kind} {} on the server, through {} to {}; {open} open",
        server.value, client_hop.value, target_hop.value
    );
    PathData {
        hops: vec![source, server, client_hop, target_hop],
        edges,
        summary,
    }
}

fn source_hop(svc: &Value, public: bool, problems: &Problems) -> Hop {
    let allow: Vec<String> = ServiceEntry::ALLOW_FROM
        .get(svc)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(plain).collect())
        .unwrap_or_default();
    let deny: Vec<String> = ServiceEntry::DENY_FROM
        .get(svc)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(plain).collect())
        .unwrap_or_default();
    let visitors: Vec<String> = ServiceEntry::VISITORS
        .get(svc)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(plain).collect())
        .unwrap_or_default();
    if public {
        Hop {
            icon: "globe",
            head: "Internet".into(),
            value: match allow.len() {
                0 => "anyone".into(),
                1 => allow[0].clone(),
                n => format!("{n} CIDRs"),
            },
            mono: !allow.is_empty(),
            dot: problems.source.as_ref().map(|_| Tone::Danger),
            unit: if !allow.is_empty() {
                "allowed sources".into()
            } else if !deny.is_empty() {
                format!("deny {}", deny.join(", "))
            } else {
                "no source limit".into()
            },
            state: if problems.source.is_some() {
                HopState::Bad
            } else {
                HopState::Normal
            },
            badge: problems.source.clone().map(|b| (b, Tone::Danger)),
        }
    } else {
        Hop {
            icon: "person",
            head: "Visitors".into(),
            value: if visitors.is_empty() {
                "none".into()
            } else {
                visitors.join(" · ")
            },
            unit: "with burrow visit".into(),
            ..Hop::default()
        }
    }
}

fn bound(svc: &Value) -> bool {
    ServiceEntry::BIND.get(svc).is_some_and(|b| !b.is_null())
}

fn website_hop(names: &[String], protocol: String, running: bool) -> Hop {
    Hop {
        icon: "box",
        head: "Server".into(),
        value: names.join(", "),
        mono: true,
        dot: Some(if running {
            Tone::Success
        } else {
            Tone::Neutral
        }),
        unit: match names.len() {
            1 => format!("by name on the HTTPS port · {protocol}"),
            n => format!("{n} names on the HTTPS port · {protocol}"),
        },
        state: HopState::Current,
        badge: None,
    }
}

fn server_hop(svc: &Value, public: bool, running: bool) -> Hop {
    let protocol = ServiceEntry::PROTOCOL
        .get(svc)
        .map(plain)
        .unwrap_or_else(|| Protocol::default().wire().into())
        .to_uppercase();
    let names = burrow_admin_logic::web::hosts(svc);
    if !bound(svc) && !names.is_empty() {
        return website_hop(&names, protocol, running);
    }
    let ports = ServiceEntry::BIND
        .get(svc)
        .and_then(burrow_admin_logic::relations::bind_ports)
        .unwrap_or(1);
    Hop {
        icon: "box",
        head: "Server".into(),
        value: if public {
            plain(ServiceEntry::BIND.get(svc).unwrap_or(&Value::Null))
        } else {
            burrow_admin_logic::web::reached(svc)
        },
        mono: public,
        dot: Some(if running {
            Tone::Success
        } else {
            Tone::Neutral
        }),
        unit: if !public {
            protocol.clone()
        } else if ports > 1 {
            format!("{ports} ports · {protocol}")
        } else {
            format!("public port · {protocol}")
        },
        state: HopState::Current,
        badge: None,
    }
}

fn target_hop(
    svc: &Value,
    problems: &Problems,
    up: usize,
    on_server: bool,
    clients: &[String],
) -> Hop {
    Hop {
        icon: "fork",
        head: "Target".into(),
        value: plain(ServiceEntry::TARGET.get(svc).unwrap_or(&Value::Null)),
        mono: true,
        dot: Some(if problems.ports.is_some() {
            Tone::Danger
        } else if up > 0 {
            Tone::Success
        } else {
            Tone::Neutral
        }),
        unit: if on_server {
            "on this server".into()
        } else {
            match clients {
                [one] => format!("on {one}"),
                _ => "on each client".into(),
            }
        },
        state: if problems.ports.is_some() {
            HopState::Bad
        } else {
            HopState::Normal
        },
        badge: problems.ports.clone().map(|b| (b, Tone::Danger)),
    }
}

fn service_edges(
    svc: &Value,
    problems: &Problems,
    up: usize,
    health_on: bool,
    many: bool,
) -> Vec<Edge> {
    let mode = if many {
        ServiceEntry::MODE
            .get(svc)
            .map(plain)
            .unwrap_or_else(|| "failover".into())
    } else {
        String::new()
    };
    let proxy = ServiceEntry::PROXY_PROTOCOL
        .get(svc)
        .filter(|p| !p.is_null())
        .map(|p| format!("PROXY {}", plain(p)))
        .unwrap_or_default();
    let interval = ServiceEntry::HEALTH_CHECK
        .get(svc)
        .and_then(|h| HealthCheck::INTERVAL_SECS.get(h))
        .map(|i| format!("every {} s", plain(i)))
        .unwrap_or_else(|| {
            if health_on {
                "every 10 s".into()
            } else {
                String::new()
            }
        });
    vec![
        Edge {
            above: String::new(),
            below: String::new(),
            state: if problems.source.is_some() {
                EdgeState::Error
            } else {
                EdgeState::Live
            },
        },
        Edge {
            above: mode,
            below: proxy,
            state: if up == 0 {
                EdgeState::Dead
            } else {
                EdgeState::Live
            },
        },
        match &problems.ports {
            Some(p) => Edge {
                above: p.clone(),
                below: "ports differ".into(),
                state: EdgeState::Error,
            },
            None => Edge {
                above: if health_on {
                    "health check".into()
                } else {
                    String::new()
                },
                below: interval,
                state: if up == 0 {
                    EdgeState::Dead
                } else {
                    EdgeState::Live
                },
            },
        },
    ]
}
