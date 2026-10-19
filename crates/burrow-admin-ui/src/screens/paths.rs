use burrow_admin_logic::rates::human_bits;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_ui_kit::{Edge, EdgeState, Hop, HopState, PathData, Tone};

use crate::form::plain;

pub fn clients_of(svc: &Value) -> Vec<String> {
    match (svc.get("client"), svc.get("clients")) {
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
) -> PathData {
    let st = status.and_then(|s| s.services.iter().find(|x| x.name == name));
    let open = st.map(|x| x.open).unwrap_or(0);
    let public = svc.get("bind").is_some_and(|b| !b.is_null());
    let allow: Vec<String> = svc
        .get("allow_from")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(plain).collect())
        .unwrap_or_default();
    let deny: Vec<String> = svc
        .get("deny_from")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(plain).collect())
        .unwrap_or_default();
    let visitors: Vec<String> = svc
        .get("visitors")
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
                "allow_from".into()
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
    let protocol = svc
        .get("protocol")
        .map(plain)
        .unwrap_or_else(|| "tcp".into())
        .to_uppercase();
    let ports = port_count(svc.get("bind")).max(1);
    let (rate, unit) = human_bits(in_rate);
    let server = Hop {
        icon: "box",
        head: "Server".into(),
        value: if public {
            plain(svc.get("bind").unwrap_or(&Value::Null))
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
    let up = clients.iter().filter(|c| online(status, c)).count();
    let healthy = st.map(|x| x.health.iter().filter(|h| h.up).count());
    let health_on = svc
        .get("health_check")
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
            clients.join(" · ")
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
        icon: "target",
        head: "Target".into(),
        value: plain(svc.get("target").unwrap_or(&Value::Null)),
        mono: true,
        dot: Some(if problems.ports.is_some() {
            Tone::Danger
        } else if up > 0 {
            Tone::Success
        } else {
            Tone::Neutral
        }),
        unit: "on each client".into(),
        state: if problems.ports.is_some() {
            HopState::Bad
        } else {
            HopState::Normal
        },
        badge: problems.ports.clone().map(|b| (b, Tone::Danger)),
    };
    let mode = if clients.len() > 1 {
        svc.get("mode")
            .map(plain)
            .unwrap_or_else(|| "failover".into())
    } else {
        String::new()
    };
    let proxy = svc
        .get("proxy_protocol")
        .filter(|p| !p.is_null())
        .map(|p| format!("PROXY {}", plain(p)))
        .unwrap_or_default();
    let interval = svc
        .get("health_check")
        .and_then(|h| h.get("interval_secs"))
        .map(|i| format!("every {} s", plain(i)))
        .unwrap_or_else(|| {
            if health_on {
                "every 10 s".into()
            } else {
                String::new()
            }
        });
    let edges = vec![
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
    let summary = format!(
        "{} {} on the server, through {} to {}; {open} open",
        if public {
            "Public port"
        } else {
            "Private service"
        },
        server.value,
        client_hop.value,
        target_hop.value
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
    let served: Vec<(String, u64)> = doc
        .get("services")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, v)| clients_of(v).iter().any(|x| x == name))
                .map(|(n, _)| {
                    let open = status
                        .and_then(|s| s.services.iter().find(|x| &x.name == n))
                        .map(|x| x.open)
                        .unwrap_or(0);
                    (n.clone(), open)
                })
                .collect()
        })
        .unwrap_or_default();
    let open: u64 = c.map(|c| c.open).unwrap_or(0);
    PathData {
        hops: vec![
            Hop {
                icon: "box",
                head: "Server".into(),
                value: listen.to_string(),
                mono: true,
                dot: Some(Tone::Success),
                unit: "listener".into(),
                ..Hop::default()
            },
            Hop {
                icon: "monitor",
                head: "Client".into(),
                value: c.and_then(|c| c.from.clone()).unwrap_or_else(|| {
                    if on {
                        name.to_string()
                    } else {
                        "offline".into()
                    }
                }),
                mono: on,
                dot: Some(if on { Tone::Success } else { Tone::Neutral }),
                unit: name.to_string(),
                state: HopState::Current,
                badge: None,
            },
            Hop {
                icon: "services",
                head: "Services".into(),
                value: format!("{} served", served.len()),
                unit: served
                    .iter()
                    .map(|s| s.0.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                ..Hop::default()
            },
        ],
        edges: vec![
            Edge {
                above: if on {
                    "connected".into()
                } else {
                    "away".into()
                },
                below: c
                    .map(|c| format!("{} idle channels", c.idle_channels))
                    .unwrap_or_default(),
                state: if on { EdgeState::Live } else { EdgeState::Dead },
            },
            Edge {
                above: format!("{open} open"),
                below: String::new(),
                state: if on { EdgeState::Live } else { EdgeState::Dead },
            },
        ],
        summary: format!(
            "Client {name} is {} and serves {} services",
            if on { "online" } else { "offline" },
            served.len()
        ),
    }
}

pub fn visitor(name: &str, doc: &Value, status: Option<&Status>) -> PathData {
    let v = status.and_then(|s| s.visitors.iter().find(|v| v.name == name));
    let open = v.map(|v| v.open).unwrap_or(0);
    let services: Vec<String> = doc
        .get("services")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, s)| {
                    s.get("visitors")
                        .and_then(Value::as_array)
                        .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(name)))
                })
                .map(|(n, _)| n.clone())
                .collect()
        })
        .unwrap_or_default();
    let admin = doc
        .get("visitors")
        .and_then(|m| m.get(name))
        .and_then(|x| x.get("admin"))
        .map(plain);
    PathData {
        hops: vec![
            Hop {
                icon: if admin.is_some() { "shield" } else { "person" },
                head: "Visitor".into(),
                value: name.to_string(),
                dot: Some(if open > 0 {
                    Tone::Success
                } else {
                    Tone::Neutral
                }),
                unit: admin
                    .map(|a| format!("admin · {a}"))
                    .unwrap_or_else(|| "visitor".into()),
                state: HopState::Current,
                ..Hop::default()
            },
            Hop {
                icon: "box",
                head: "Server".into(),
                value: "burrow".into(),
                unit: "checks access".into(),
                ..Hop::default()
            },
            Hop {
                icon: "services",
                head: "Services".into(),
                value: format!("{} it may use", services.len()),
                unit: services.join(", "),
                ..Hop::default()
            },
        ],
        edges: vec![
            Edge {
                above: format!("{open} open"),
                below: String::new(),
                state: if open > 0 {
                    EdgeState::Live
                } else {
                    EdgeState::Dead
                },
            },
            Edge {
                above: String::new(),
                below: String::new(),
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
}
