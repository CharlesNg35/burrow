use burrow_admin_logic::rates::human_bits;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{Entry, HealthCheck, Protocol, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{Edge, EdgeState, Hop, HopState, PathData, Tone};

use crate::form::plain;

pub fn clients_of(svc: &Value) -> Vec<String> {
    match (
        ServiceEntry::CLIENT.get(svc),
        ServiceEntry::CLIENTS.get(svc),
    ) {
        (Some(Value::String(c)), _) => vec![c.clone()],
        (_, Some(Value::Array(a))) => a.iter().map(plain).collect(),
        (_, Some(Value::String(c))) => vec![c.clone()],
        _ => Vec::new(),
    }
}

pub fn port_count(v: Option<&Value>) -> u32 {
    let Some(v) = v else { return 0 };
    let s = plain(v);
    if s.starts_with("unix:") {
        return 1;
    }
    let port = s.rsplit_once(':').map_or(s.as_str(), |(_, p)| p);
    match port.split_once('-') {
        Some((a, b)) => match (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
            (Ok(a), Ok(b)) if b >= a => b - a + 1,
            _ => 1,
        },
        None => 1,
    }
}

pub fn online(status: Option<&Status>, client: &str) -> bool {
    status
        .and_then(|s| s.clients.iter().find(|c| c.name == client))
        .is_some_and(|c| c.connected)
}

pub struct Problems {
    pub source: Option<String>,
    pub clients: Option<String>,
    pub ports: Option<String>,
}

pub fn service(
    name: &str,
    svc: &Value,
    status: Option<&Status>,
    in_rate: f64,
    problems: &Problems,
    editing: bool,
    reserved: &[String],
) -> PathData {
    let st = status.and_then(|s| s.services.iter().find(|x| x.name == name));
    let open = st.map(|x| x.open).unwrap_or(0);
    let public = ServiceEntry::BIND.get(svc).is_some_and(|b| !b.is_null());
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
    let source = if public {
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
                ServiceEntry::ALLOW_FROM.key().into()
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
    };
    let protocol = ServiceEntry::PROTOCOL
        .get(svc)
        .map(plain)
        .unwrap_or_else(|| Protocol::default().wire().into())
        .to_uppercase();
    let ports = port_count(ServiceEntry::BIND.get(svc)).max(1);
    let (rate, unit) = human_bits(in_rate);
    let server = Hop {
        icon: "box",
        head: "Server".into(),
        value: if public {
            plain(ServiceEntry::BIND.get(svc).unwrap_or(&Value::Null))
        } else {
            "private".into()
        },
        mono: public,
        dot: Some(if st.is_some() {
            Tone::Success
        } else {
            Tone::Neutral
        }),
        unit: if ports > 1 {
            format!("{ports} ports · {protocol}")
        } else {
            format!("bound · {protocol}")
        },
        state: HopState::Current,
        badge: None,
    };
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
    let target_hop = Hop {
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
            "on each client".into()
        },
        state: if problems.ports.is_some() {
            HopState::Bad
        } else {
            HopState::Normal
        },
        badge: problems.ports.clone().map(|b| (b, Tone::Danger)),
    };
    let mode = if clients.len() > 1 {
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
    let mut edges = vec![
        Edge {
            above: format!("{open} open"),
            below: format!("{rate} {unit} in"),
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
    ];
    let kind = if public {
        "Public port"
    } else {
        "Private service"
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

pub fn client(name: &str, doc: &Value, status: Option<&Status>, listen: &str) -> PathData {
    let c = status.and_then(|s| s.clients.iter().find(|c| c.name == name));
    let on = c.is_some_and(|c| c.connected);
    let served: Vec<(String, String)> = ServiceEntry::entries(doc)
        .map(|m| {
            m.iter()
                .filter(|(_, v)| clients_of(v).iter().any(|x| x == name))
                .map(|(n, v)| {
                    let at = ServiceEntry::BIND.get(v).filter(|b| !b.is_null()).map(|b| {
                        let t = plain(b);
                        match t.rsplit_once(':') {
                            Some((_, p)) => format!(":{p}"),
                            None => format!(":{t}"),
                        }
                    });
                    (n.clone(), at.unwrap_or_else(|| "private".into()))
                })
                .collect()
        })
        .unwrap_or_default();
    let open: u64 = c.map(|c| c.open).unwrap_or(0);
    let short = match listen.rsplit_once(':') {
        Some((_, p)) => format!(":{p}"),
        None => listen.to_string(),
    };
    let first = served.first().cloned();
    PathData {
        hops: vec![
            Hop {
                icon: "monitor",
                head: "Client".into(),
                value: name.to_string(),
                mono: true,
                dot: Some(if on { Tone::Success } else { Tone::Neutral }),
                unit: c
                    .and_then(|c| c.from.clone())
                    .map(|f| f.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or(f))
                    .unwrap_or_else(|| {
                        if on {
                            String::new()
                        } else {
                            "not connected".into()
                        }
                    }),
                state: HopState::Current,
                badge: None,
            },
            Hop {
                icon: "box",
                head: "Server".into(),
                value: short,
                mono: true,
                dot: Some(Tone::Success),
                unit: "this server".into(),
                ..Hop::default()
            },
            Hop {
                icon: "stack",
                head: "Services".into(),
                value: first
                    .as_ref()
                    .map(|(n, at)| format!("{n} {at}"))
                    .unwrap_or_else(|| "none".into()),
                mono: true,
                dot: first
                    .as_ref()
                    .map(|_| if on { Tone::Success } else { Tone::Warning }),
                unit: served
                    .iter()
                    .skip(1)
                    .map(|(n, at)| {
                        if at == "private" {
                            format!("{n} · private")
                        } else {
                            n.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                ..Hop::default()
            },
        ],
        edges: vec![
            Edge {
                above: if on {
                    "tunnel".into()
                } else {
                    "not connected".into()
                },
                below: c
                    .filter(|_| on)
                    .map(|c| format!("{} idle · {} open", c.idle_channels, open))
                    .unwrap_or_default(),
                state: if on { EdgeState::Live } else { EdgeState::Dead },
            },
            Edge {
                above: "serves".into(),
                below: format!(
                    "{} service{}",
                    served.len(),
                    if served.len() == 1 { "" } else { "s" }
                ),
                state: if on && !served.is_empty() {
                    EdgeState::Live
                } else {
                    EdgeState::Dead
                },
            },
        ],
        summary: format!(
            "Client {name} is {} and serves {} services",
            if on { "online" } else { "offline" },
            served.len()
        ),
    }
}

pub fn visitor(
    name: &str,
    doc: &Value,
    status: Option<&Status>,
    me: Option<(&str, &str)>,
) -> PathData {
    let v = status.and_then(|s| s.visitors.iter().find(|v| v.name == name));
    let open = v.map(|v| v.open).unwrap_or(0);
    let entry = VisitorEntry::entry(doc, name);
    let services: Vec<(String, bool)> = ServiceEntry::entries(doc)
        .map(|m| {
            m.iter()
                .filter(|(_, s)| {
                    ServiceEntry::VISITORS
                        .get(s)
                        .and_then(Value::as_array)
                        .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(name)))
                })
                .map(|(n, s)| {
                    (
                        n.clone(),
                        status.is_none_or(|st| clients_of(s).iter().any(|c| online(Some(st), c))),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let down = services.iter().filter(|(_, up)| !up).count();
    let admin_from: Vec<String> = entry
        .and_then(|x| VisitorEntry::ADMIN_FROM.get(x))
        .and_then(Value::as_array)
        .map(|a| a.iter().map(plain).collect())
        .unwrap_or_default();
    let this_session = me.is_some_and(|(n, _)| n == name);
    PathData {
        hops: vec![
            Hop {
                icon: "person",
                head: "Visitor".into(),
                value: name.to_string(),
                mono: true,
                unit: if this_session {
                    format!("this session · {open} open")
                } else if open > 0 {
                    format!("active · {open} open")
                } else {
                    "idle".into()
                },
                state: HopState::Current,
                ..Hop::default()
            },
            Hop {
                icon: "box",
                head: "Server".into(),
                value: "this server".into(),
                dot: Some(Tone::Success),
                unit: match entry.and_then(|x| VisitorEntry::ADMIN.get(x)).map(plain) {
                    Some(_) if !admin_from.is_empty() => {
                        format!("admin from {}", admin_from.join(", "))
                    }
                    Some(_) => "admin from any address".into(),
                    None => "checks access".into(),
                },
                ..Hop::default()
            },
            Hop {
                icon: "stack",
                head: "Services".into(),
                value: if services.is_empty() {
                    "none".into()
                } else {
                    services
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(" · ")
                },
                dot: (!services.is_empty()).then_some(if down > 0 {
                    Tone::Warning
                } else {
                    Tone::Success
                }),
                unit: services
                    .iter()
                    .filter(|(_, up)| !up)
                    .map(|(n, _)| format!("{n} down"))
                    .collect::<Vec<_>>()
                    .join(", "),
                state: if down > 0 {
                    HopState::Warn
                } else {
                    HopState::Normal
                },
                badge: (down > 0).then(|| (format!("{down} down"), Tone::Danger)),
                mono: false,
            },
        ],
        edges: vec![
            Edge {
                above: "burrow visit".into(),
                below: format!("{open} open"),
                state: if open > 0 {
                    EdgeState::Live
                } else {
                    EdgeState::Dead
                },
            },
            Edge {
                above: "may use".into(),
                below: format!(
                    "{} service{}",
                    services.len(),
                    if services.len() == 1 { "" } else { "s" }
                ),
                state: if services.is_empty() {
                    EdgeState::Dead
                } else {
                    EdgeState::Live
                },
            },
        ],
        summary: format!(
            "Visitor {name} may use {} services; {open} open visits",
            services.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn a_service_path_reads_the_config_and_marks_problems() {
        let svc = json!({"clients": ["a", "b"], "mode": "balance", "bind": "0.0.0.0:30000-30010", "target": "10.0.0.5:30000-30005", "allow_from": ["10.1.0.300/16"]});
        let p = service(
            "media",
            &svc,
            None,
            0.0,
            &Problems {
                source: Some("invalid CIDR".into()),
                clients: None,
                ports: Some("6 ≠ 11 ports".into()),
            },
            true,
            &[],
        );
        assert_eq!(p.hops.len(), 4);
        assert_eq!(p.hops[0].state, HopState::Bad);
        assert_eq!(p.hops[1].unit, "11 ports · TCP");
        assert_eq!(p.hops[2].value, "a · b");
        assert_eq!(p.edges[1].above, "balance");
        assert_eq!(p.edges[2].state, EdgeState::Error);
        assert_eq!(p.hops[3].badge.as_ref().unwrap().0, "6 ≠ 11 ports");
        assert_eq!(port_count(Some(&json!("unix:/run/x.sock"))), 1);
        assert_eq!(port_count(Some(&json!(7840))), 1);
    }

    #[test]
    fn a_service_on_this_server_goes_from_the_server_straight_to_its_target() {
        let none = Problems {
            source: None,
            clients: None,
            ports: None,
        };
        let reserved = ["self".to_string()];
        let svc = json!({"client": "self", "bind": 8080, "target": "127.0.0.1:3000", "proxy_protocol": "v2"});
        let p = service("web", &svc, None, 0.0, &none, false, &reserved);
        let heads: Vec<&str> = p.hops.iter().map(|h| h.head.as_str()).collect();
        assert_eq!(heads, ["Internet", "Server", "Target"]);
        assert_eq!(p.edges.len(), 2);
        assert_eq!(p.edges[1].state, EdgeState::Live);
        assert_eq!(p.edges[1].below, "PROXY v2");
        assert_eq!(p.hops[2].unit, "on this server");
        assert_eq!(
            p.summary,
            "Public port 8080 on the server, to 127.0.0.1:3000 on this server; 0 open"
        );
        let mixed = json!({"clients": ["self", "home"], "bind": 8080, "target": 3000});
        let p = service("web", &mixed, None, 0.0, &none, false, &reserved);
        assert_eq!(p.hops.len(), 4);
        assert_eq!(p.hops[2].value, "This server · home");
        assert_eq!(p.hops[2].unit, "1 of 2 online");
    }
}
