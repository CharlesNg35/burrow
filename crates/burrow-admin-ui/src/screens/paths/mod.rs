mod service;

use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{Entry, Protocol, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{Edge, EdgeState, Hop, HopState, PathData, Tone};

use crate::form::plain;

pub(crate) use service::service;

pub fn clients_of(svc: &Value) -> Vec<String> {
    ServiceEntry::client_names(svc)
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

pub fn client(name: &str, doc: &Value, status: Option<&Status>, listen: &str) -> PathData {
    let c = status.and_then(|s| s.clients.iter().find(|c| c.name == name));
    let on = c.is_some_and(|c| c.connected);
    let served: Vec<(String, String)> = ServiceEntry::entries(doc)
        .map(|m| {
            m.iter()
                .filter(|(_, v)| clients_of(v).iter().any(|x| x == name))
                .map(|(n, v)| {
                    let at = ServiceEntry::BIND
                        .get(v)
                        .filter(|b| !b.is_null())
                        .map(|b| {
                            let t = plain(b);
                            match t.rsplit_once(':') {
                                Some((_, p)) => format!(":{p}"),
                                None => format!(":{t}"),
                            }
                        })
                        .unwrap_or_else(|| burrow_admin_logic::web::reached(v));
                    (n.clone(), at)
                })
                .collect()
        })
        .unwrap_or_default();
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
                    .unwrap_or_default(),
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
                value: count_of(served.len(), "service"),
                mono: false,
                dot: first
                    .as_ref()
                    .map(|_| if on { Tone::Success } else { Tone::Warning }),
                ..Hop::default()
            },
        ],
        edges: vec![
            Edge {
                above: "tunnel".into(),
                below: String::new(),
                state: if on { EdgeState::Live } else { EdgeState::Dead },
            },
            Edge {
                above: "serves".into(),
                below: String::new(),
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

fn unseen(name: &str) -> String {
    let added = crate::store::try_store().and_then(|s| s.added_secs(VisitorEntry::section(), name));
    match added {
        Some(a) => format!("added {}, not seen yet", crate::format::ago(a)),
        None => "not seen since start".into(),
    }
}

fn count_of(n: usize, what: &str) -> String {
    match n {
        0 => "none".into(),
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
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
    let services: Vec<(String, bool, bool)> = ServiceEntry::entries(doc)
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
                        udp(s),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let down = services.iter().filter(|(_, up, _)| !up).count();
    let udp_count = services.iter().filter(|(_, _, u)| *u).count();
    let admin_from: Vec<String> = entry
        .and_then(|x| VisitorEntry::ADMIN_FROM.get(x))
        .and_then(Value::as_array)
        .map(|a| a.iter().map(plain).collect())
        .unwrap_or_default();
    let this_session = me.is_some_and(|(n, _)| n == name);
    let reach_none = entry
        .and_then(|x| VisitorEntry::REACH.get(x))
        .and_then(Value::as_object)
        .is_none_or(|m| m.is_empty());
    PathData {
        hops: vec![
            Hop {
                icon: "person",
                head: "Visitor".into(),
                value: name.to_string(),
                mono: true,
                unit: match (this_session, v) {
                    (true, _) => "you, in burrow admin now".into(),
                    (false, Some(_)) if open > 0 => format!("in use now · {open} open"),
                    (false, Some(v)) => match v.last_seen_secs {
                        Some(a) => format!("seen {} · no open connection", crate::format::ago(a)),
                        None => unseen(name),
                    },
                    (false, None) if status.is_some() => unseen(name),
                    _ => String::new(),
                },
                dot: Some(if this_session || open > 0 {
                    Tone::Success
                } else {
                    Tone::Neutral
                }),
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
                value: count_of(services.len(), "service"),
                dot: (!services.is_empty()).then_some(if down > 0 {
                    Tone::Warning
                } else {
                    Tone::Success
                }),
                unit: if udp_count > 0 {
                    format!("{} TCP · {udp_count} UDP", services.len() - udp_count)
                } else {
                    String::new()
                },
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
                above: if services.is_empty()
                    && reach_none
                    && entry.and_then(|x| VisitorEntry::ADMIN.get(x)).is_some()
                {
                    "burrow admin".into()
                } else {
                    "burrow visit".into()
                },
                below: String::new(),
                state: if open > 0 || this_session {
                    EdgeState::Live
                } else {
                    EdgeState::Dead
                },
            },
            Edge {
                above: "may use".into(),
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

pub fn udp(svc: &Value) -> bool {
    ServiceEntry::PROTOCOL
        .get(svc)
        .and_then(Value::as_str)
        .and_then(Protocol::from_wire)
        == Some(Protocol::Udp)
}

pub const PICK_VISITORS: &str =
    "Turn on a way in: Public port, Website, or Visitors with at least one visitor";

pub fn add_blocker(svc: &Value) -> Option<&'static str> {
    let public = burrow_admin_logic::web::is_public(svc);
    let picked = ServiceEntry::VISITORS
        .get(svc)
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    (!public && !picked).then_some(PICK_VISITORS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn a_visitor_never_seen_has_a_hollow_dot_and_one_in_use_a_green_one() {
        let doc = json!({"visitors": {"v": {}}});
        let mut st: Status = burrow_admin_proto::serde_json::from_value(json!({
            "version": "0.1.0", "uptime_secs": 10, "listening": [], "handshakes_refused": 0, "udp_peers": 0,
            "clients": [], "services": [], "visitors": [{"name": "v", "open": 0}]
        }))
        .unwrap();
        assert_eq!(
            visitor("v", &doc, Some(&st), None).hops[0].dot,
            Some(Tone::Neutral)
        );
        st.visitors[0].open = 1;
        assert_eq!(
            visitor("v", &doc, Some(&st), None).hops[0].dot,
            Some(Tone::Success)
        );
    }

    #[test]
    fn a_private_service_of_either_protocol_needs_a_visitor_before_it_can_be_added() {
        assert_eq!(
            add_blocker(&json!({"client": "a", "target": 80})),
            Some(PICK_VISITORS)
        );
        assert_eq!(
            add_blocker(&json!({"client": "a", "target": 80, "visitors": []})),
            Some(PICK_VISITORS)
        );
        assert_eq!(
            add_blocker(&json!({"client": "a", "target": 80, "visitors": ["ops"]})),
            None
        );
        assert_eq!(
            add_blocker(&json!({"client": "a", "target": 80, "bind": 7840})),
            None
        );
        assert_eq!(
            add_blocker(&json!({"client": "a", "target": 443, "web": "nas.example.com"})),
            None,
            "a website is reached by its name"
        );
        assert_eq!(
            add_blocker(&json!({"client": "a", "target": 53, "protocol": "udp"})),
            Some(PICK_VISITORS)
        );
        assert_eq!(
            add_blocker(
                &json!({"client": "a", "target": 53, "protocol": "udp", "visitors": ["ops"]})
            ),
            None
        );
        assert_eq!(
            add_blocker(&json!({"client": "a", "target": 53, "protocol": "udp", "bind": 5353})),
            None
        );
    }

    #[test]
    fn a_service_path_reads_the_config_and_marks_problems() {
        let svc = json!({"clients": ["a", "b"], "mode": "balance", "bind": "0.0.0.0:30000-30010", "target": "10.0.0.5:30000-30005", "allow_from": ["10.1.0.300/16"]});
        let p = service(
            "media",
            &svc,
            None,
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
        assert_eq!(p.edges[1].above, "Balance");
        assert_eq!(p.edges[2].state, EdgeState::Error);
        assert_eq!(p.hops[3].badge.as_ref().unwrap().0, "6 ≠ 11 ports");
        assert!(
            p.edges[0].above.is_empty() && p.edges[0].below.is_empty(),
            "counts and rates live in the stat cards, not on the path"
        );
        let private = json!({"client": "a", "target": 22, "visitors": ["lap"]});
        let p = service(
            "ssh",
            &private,
            None,
            &Problems {
                source: None,
                clients: None,
                ports: None,
            },
            false,
            &[],
        );
        assert_eq!(p.hops[1].value, burrow_admin_logic::web::VISITORS_ONLY);
        assert_eq!(p.hops[1].unit, "TCP");
    }

    #[test]
    fn a_private_udp_service_goes_from_its_visitors_through_the_server() {
        let none = Problems {
            source: None,
            clients: None,
            ports: None,
        };
        let dns =
            json!({"client": "a", "protocol": "udp", "target": 53, "visitors": ["lap", "ops"]});
        let p = service("dns", &dns, None, &none, false, &[]);
        assert_eq!(p.hops[0].head, "Visitors");
        assert_eq!(p.hops[0].value, "lap · ops");
        assert_eq!(p.hops[1].value, burrow_admin_logic::web::VISITORS_ONLY);
        assert_eq!(p.hops[1].unit, "UDP");
    }

    #[test]
    fn a_visitor_path_counts_its_tcp_and_udp_services() {
        let doc = json!({
            "visitors": {"lap": {}},
            "services": {
                "db": {"client": "a", "target": 5432, "visitors": ["lap"]},
                "dns": {"client": "a", "protocol": "udp", "target": 53, "visitors": ["lap"]},
                "game": {"client": "a", "protocol": "udp", "target": 27015, "visitors": ["lap"]}
            }
        });
        let p = visitor("lap", &doc, None, None);
        assert_eq!(p.hops[2].value, "3 services");
        assert_eq!(p.hops[2].unit, "1 TCP · 2 UDP");
        let tcp_only = json!({
            "visitors": {"lap": {}},
            "services": {"db": {"client": "a", "target": 5432, "visitors": ["lap"]}}
        });
        assert_eq!(visitor("lap", &tcp_only, None, None).hops[2].unit, "");
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
        let p = service("web", &svc, None, &none, false, &reserved);
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
        let p = service("web", &mixed, None, &none, false, &reserved);
        assert_eq!(p.hops.len(), 4);
        assert_eq!(p.hops[2].value, "This server · home");
        assert_eq!(p.hops[2].unit, "1 of 2 online");
    }
}
