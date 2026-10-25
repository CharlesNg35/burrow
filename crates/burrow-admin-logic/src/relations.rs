use burrow_admin_proto::schema::{Cond, Node, Relation};
use burrow_admin_proto::serde_json::Value;

use crate::tree::{Lookup, fields, lookup};

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(n), Value::String(s)) | (Value::String(s), Value::Number(n)) => {
            s.trim().parse::<f64>().ok() == n.as_f64()
        }
        _ => a == b,
    }
}

fn len(v: &Value) -> u64 {
    match v {
        Value::Array(a) => a.len() as u64,
        Value::Object(m) => m.len() as u64,
        Value::Null => 0,
        _ => 1,
    }
}

fn tri(c: &Cond, get: &dyn Fn(&str) -> Lookup) -> Option<bool> {
    match c {
        Cond::Eq(p, x) => match get(p) {
            Lookup::Value(v) => Some(same(&v, x)),
            Lookup::Unset => Some(x.is_null()),
            Lookup::Unknown => None,
        },
        Cond::Ne(p, x) => tri(&Cond::Eq(p.clone(), x.clone()), get).map(|b| !b),
        Cond::LenGe(p, n) => match get(p) {
            Lookup::Value(v) => Some(len(&v) >= *n),
            Lookup::Unset => Some(*n == 0),
            Lookup::Unknown => None,
        },
        Cond::Set(p) => match get(p) {
            Lookup::Value(v) => Some(!v.is_null()),
            Lookup::Unset => Some(false),
            Lookup::Unknown => None,
        },
        Cond::All(cs) => {
            let r: Vec<Option<bool>> = cs.iter().map(|c| tri(c, get)).collect();
            if r.contains(&Some(false)) {
                Some(false)
            } else if r.iter().all(Option::is_some) {
                Some(true)
            } else {
                None
            }
        }
        Cond::Any(cs) => {
            let r: Vec<Option<bool>> = cs.iter().map(|c| tri(c, get)).collect();
            if r.contains(&Some(true)) {
                Some(true)
            } else if r.iter().all(Option::is_some) {
                Some(false)
            } else {
                None
            }
        }
        Cond::Not(c) => tri(c, get).map(|b| !b),
        Cond::Unknown(_) => None,
    }
}

pub fn holds(c: &Cond, get: &dyn Fn(&str) -> Lookup) -> bool {
    tri(c, get).unwrap_or(true)
}

fn holds_at(c: &Cond, root: &Node, doc: &Value, base: &[String]) -> bool {
    holds(c, &|p| lookup(root, doc, base, p))
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FieldState {
    pub visible: bool,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub warnings: Vec<String>,
    pub default_from: Option<Value>,
}

pub fn field_state(node: &Node, root: &Node, doc: &Value, base: &[String]) -> FieldState {
    let mut s = FieldState {
        visible: true,
        enabled: node.editable.is_yes(),
        ..FieldState::default()
    };
    for r in &node.relations {
        match r {
            Relation::VisibleIf(c) => s.visible &= holds_at(c, root, doc, base),
            Relation::EnabledIf { cond, reason } => {
                if !holds_at(cond, root, doc, base) {
                    s.enabled = false;
                    s.disabled_reason.get_or_insert_with(|| reason.clone());
                }
            }
            Relation::WarnIf { cond, message } => {
                if !matches!(cond, Cond::Unknown(_)) && holds_at(cond, root, doc, base) {
                    s.warnings.push(message.clone());
                }
            }
            Relation::DefaultFrom(p) => {
                if let Lookup::Value(v) = lookup(root, doc, base, p) {
                    s.default_from = Some(v);
                }
            }
            _ => {}
        }
    }
    s
}

fn set_in(v: &Value, key: &str) -> bool {
    v.get(key).is_some_and(|x| !x.is_null())
}

pub fn entity_problems(entity: &Node, value: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for f in fields(entity) {
        for r in &f.node.relations {
            match r {
                Relation::OneOf { group } => match groups.iter_mut().find(|(g, _)| g == group) {
                    Some((_, keys)) => keys.push(f.key.clone()),
                    None => groups.push((group.clone(), vec![f.key.clone()])),
                },
                Relation::SameLen(other) => {
                    let count = |k: &str| value.get(k).and_then(|v| port_count(k, v));
                    if let (Some(a), Some(b)) = (count(&f.key), count(other))
                        && a != b
                    {
                        out.push((
                            f.key.clone(),
                            format!(
                                "{a} {} here, but {other} has {b}",
                                if a == 1 { "port" } else { "ports" }
                            ),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    for (_, keys) in groups {
        let set: Vec<&String> = keys.iter().filter(|k| set_in(value, k)).collect();
        let names = keys.join(" or ");
        match set.len() {
            0 => out.push((keys[0].clone(), format!("set {names}"))),
            1 => {}
            _ => {
                for k in set {
                    out.push((k.clone(), format!("set only one of {names}")));
                }
            }
        }
    }
    out
}

pub fn bind_ports(v: &Value) -> Option<u16> {
    let bind: burrow_config::Bind = burrow_admin_proto::serde_json::from_value(v.clone()).ok()?;
    bind.ranged([0, 0, 0, 0], burrow_config::ServiceEntry::BIND.key())
        .ok()
        .map(|(_, n)| n)
}

pub fn target_ports(v: &Value) -> Option<u16> {
    let target: burrow_config::Target =
        burrow_admin_proto::serde_json::from_value(v.clone()).ok()?;
    target.ports().ok()
}

fn port_count(key: &str, v: &Value) -> Option<u16> {
    if key == burrow_config::ServiceEntry::TARGET.key() {
        target_ports(v)
    } else {
        bind_ports(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{doc, schema};
    use burrow_admin_proto::serde_json::json;

    fn p(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    fn get(m: Value) -> impl Fn(&str) -> Lookup {
        move |k| match m.get(k) {
            Some(Value::Null) => Lookup::Unset,
            Some(v) => Lookup::Value(v.clone()),
            None => Lookup::Unknown,
        }
    }

    #[test]
    fn the_condition_grammar() {
        let g = get(json!({"protocol": "udp", "clients": ["a", "b"], "port": 80, "none": null}));
        let c = |v: Value| Cond::from_json(&v);
        assert!(!holds(&c(json!({"eq": ["protocol", "tcp"]})), &g));
        assert!(holds(&c(json!({"ne": ["protocol", "tcp"]})), &g));
        assert!(holds(&c(json!({"eq": ["port", "80"]})), &g));
        assert!(holds(&c(json!({"len_ge": ["clients", 2]})), &g));
        assert!(!holds(&c(json!({"len_ge": ["clients", 3]})), &g));
        assert!(!holds(&c(json!({"set": "none"})), &g));
        assert!(holds(
            &c(json!({"all": [{"set": "protocol"}, {"not": {"set": "none"}}]})),
            &g
        ));
        assert!(!holds(
            &c(json!({"any": [{"set": "none"}, {"eq": ["protocol", "tcp"]}]})),
            &g
        ));
    }

    #[test]
    fn unknown_operators_and_fields_show_and_enable() {
        let g = get(json!({"protocol": "udp"}));
        let c = |v: Value| Cond::from_json(&v);
        assert!(holds(&c(json!({"geo_within": ["x", 1]})), &g));
        assert!(holds(&c(json!({"not": {"geo_within": 1}})), &g));
        assert!(holds(&c(json!({"eq": ["future", 1]})), &g));
        assert!(holds(&c(json!({"not": {"eq": ["future", 1]}})), &g));
        assert!(!holds(
            &c(json!({"all": [{"eq": ["future", 1]}, {"eq": ["protocol", "tcp"]}]})),
            &g
        ));
        assert!(holds(
            &c(json!({"any": [{"eq": ["future", 1]}, {"eq": ["protocol", "tcp"]}]})),
            &g
        ));
    }

    #[test]
    fn tcp_only_fields_are_disabled_on_udp_services() {
        let s = schema();
        let d = doc();
        let svc = s.root.at(&p(&["services", "x"])).expect("service");
        let hc = &svc.field("health_check").expect("field").node;
        let web = field_state(hc, &s.root, &d, &p(&["services", "web"]));
        assert!(web.enabled && web.visible);
        let dns = field_state(hc, &s.root, &d, &p(&["services", "dns"]));
        assert!(!dns.enabled);
        assert_eq!(
            dns.disabled_reason.as_deref(),
            Some("UDP services do not support this")
        );
        let mode = &svc.field("mode").expect("field").node;
        let one = field_state(mode, &s.root, &d, &p(&["services", "web"]));
        assert!(one.visible && !one.enabled, "shown, with its reason");
        assert_eq!(
            one.disabled_reason.as_deref(),
            Some("This applies only with two or more clients")
        );
        assert!(field_state(mode, &s.root, &d, &p(&["services", "both"])).enabled);
        let vis = &s
            .root
            .at(&p(&["visitors", "x"]))
            .expect("visitor")
            .field("admin_from")
            .expect("f")
            .node;
        let laptop = field_state(vis, &s.root, &d, &p(&["visitors", "laptop"]));
        assert!(laptop.visible && !laptop.enabled);
    }

    #[test]
    fn default_from_and_inverse_filters() {
        let s = schema();
        let mut d = doc();
        d["limits"] = json!({"max_connections_per_client": 50});
        let mc = &s
            .root
            .at(&p(&["clients", "x", "max_connections"]))
            .expect("node")
            .clone();
        let st = field_state(mc, &s.root, &d, &p(&["clients", "home"]));
        assert_eq!(st.default_from, Some(json!(50)));
    }

    #[test]
    fn one_of_and_same_len_are_checked_on_the_draft() {
        let s = schema();
        let svc = s.root.at(&p(&["services", "x"])).expect("service");
        assert!(entity_problems(svc, &json!({"client": "a", "bind": 1, "target": 2})).is_empty());
        let both = entity_problems(
            svc,
            &json!({"client": "a", "clients": ["a", "b"], "target": 2}),
        );
        assert_eq!(both.len(), 2, "{both:?}");
        assert!(both[0].1.contains("only one of client or clients"));
        let none = entity_problems(svc, &json!({"target": 2}));
        assert_eq!(
            none,
            vec![("client".into(), "set client or clients".into())]
        );
        let ranges = entity_problems(
            svc,
            &json!({"client": "a", "bind": "7000-7002", "target": "3000-3001"}),
        );
        assert_eq!(
            ranges,
            vec![("target".into(), "2 ports here, but bind has 3".into())]
        );
        assert_eq!(
            entity_problems(
                svc,
                &json!({"client": "a", "bind": "7000-7002", "target": 3000})
            ),
            vec![("target".into(), "1 port here, but bind has 3".into())],
            "validate refuses it too (range-mismatch)"
        );
    }
}
