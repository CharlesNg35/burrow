use burrow_admin_proto::schema::{Entity, Node};
use burrow_config::{ClientEntry, VisitorEntry, entity_of_section};

pub const OVERVIEW: &str = "overview";
pub const SETTINGS: &str = "settings";
pub const ACCESS: &str = "access";
pub const SECRETS: &str = "secrets";
pub const CONNECT: &str = "connect";

const VISITOR_ACCESS: [&str; 4] = [
    VisitorEntry::ADMIN.key(),
    VisitorEntry::ADMIN_FROM.key(),
    VisitorEntry::REACH.key(),
    VisitorEntry::MAX_CONNECTIONS.key(),
];

pub fn tab_of(section: &str, key: &str) -> &'static str {
    match entity_of_section(section) {
        Some(Entity::Client) if key == ClientEntry::SECRET.key() => SECRETS,
        Some(Entity::Visitor) if key == VisitorEntry::SECRET.key() => SECRETS,
        Some(Entity::Visitor) if VISITOR_ACCESS.contains(&key) => ACCESS,
        _ => SETTINGS,
    }
}

pub fn placed(section: &str, node: &Node) -> Vec<(&'static str, Vec<String>)> {
    let mut out: Vec<(&'static str, Vec<String>)> = Vec::new();
    for f in burrow_admin_logic::tree::fields(node) {
        let tab = tab_of(section, &f.key);
        match out.iter_mut().find(|(t, _)| *t == tab) {
            Some((_, keys)) => keys.push(f.key.clone()),
            None => out.push((tab, vec![f.key.clone()])),
        }
    }
    out
}

pub fn tabs(section: &str, node: &Node) -> Vec<(&'static str, String)> {
    let has = |tab: &str| {
        placed(section, node)
            .iter()
            .any(|(t, k)| *t == tab && !k.is_empty())
    };
    let label = |t: &'static str| {
        let l = match t {
            OVERVIEW => "Overview",
            SETTINGS => "Settings",
            ACCESS => "Access",
            SECRETS => "Secrets",
            _ => "Connect",
        };
        (t, l.to_string())
    };
    let order: &[&'static str] = match entity_of_section(section) {
        Some(Entity::Client) => &[OVERVIEW, SECRETS, SETTINGS, CONNECT],
        Some(Entity::Visitor) => &[ACCESS, SECRETS, CONNECT, SETTINGS],
        _ => &[OVERVIEW, SETTINGS],
    };
    order
        .iter()
        .filter(|t| matches!(**t, OVERVIEW | CONNECT) || has(t))
        .map(|t| label(t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::schema::SemType;
    use burrow_config::admin_schema::AdminSchema;
    use burrow_config::{Entry, ServiceEntry};

    fn entity_node(root: &Node, section: &str) -> Node {
        root.at(&[section.to_string(), "*".to_string()])
            .cloned()
            .unwrap_or_else(|| panic!("{section} in the schema"))
    }

    #[test]
    fn every_entity_field_of_the_live_schema_lands_in_exactly_one_tab() {
        let root = <burrow_config::ServerConfig as AdminSchema>::node();
        for section in [
            ServiceEntry::section(),
            ClientEntry::section(),
            VisitorEntry::section(),
        ] {
            let node = entity_node(&root, section);
            let keys: Vec<String> = burrow_admin_logic::tree::fields(&node)
                .iter()
                .map(|f| f.key.clone())
                .collect();
            assert!(!keys.is_empty(), "{section}");
            let placed = placed(section, &node);
            let shown: Vec<&'static str> = tabs(section, &node).iter().map(|(t, _)| *t).collect();
            for key in &keys {
                let homes: Vec<&str> = placed
                    .iter()
                    .filter(|(_, k)| k.contains(key))
                    .map(|(t, _)| *t)
                    .collect();
                assert_eq!(homes.len(), 1, "{section}.{key} is placed in {homes:?}");
                assert!(
                    shown.contains(&homes[0]),
                    "{section}.{key} sits in a tab that is shown"
                );
            }
            let total: usize = placed.iter().map(|(_, k)| k.len()).sum();
            assert_eq!(total, keys.len(), "{section}");
        }
    }

    #[test]
    fn a_field_a_newer_server_adds_falls_into_settings() {
        let root = <burrow_config::ServerConfig as AdminSchema>::node();
        let mut node = entity_node(&root, VisitorEntry::section());
        if let SemType::Object(o) = &mut node.sem {
            o.fields.push(burrow_admin_proto::schema::Field {
                key: "future_knob".into(),
                node: Node::new(SemType::Bool),
            });
        }
        assert_eq!(tab_of(VisitorEntry::section(), "future_knob"), SETTINGS);
        let shown: Vec<&'static str> = tabs(VisitorEntry::section(), &node)
            .iter()
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(shown, [ACCESS, SECRETS, CONNECT, SETTINGS]);
        let today: Vec<&'static str> = tabs(
            VisitorEntry::section(),
            &entity_node(&root, VisitorEntry::section()),
        )
        .iter()
        .map(|(t, _)| *t)
        .collect();
        assert_eq!(
            today,
            [ACCESS, SECRETS, CONNECT],
            "no empty Settings tab today"
        );
        let services: Vec<&'static str> = tabs(
            ServiceEntry::section(),
            &entity_node(&root, ServiceEntry::section()),
        )
        .iter()
        .map(|(t, _)| *t)
        .collect();
        assert_eq!(services, [OVERVIEW, SETTINGS]);
    }

    fn helps(v: &burrow_admin_proto::serde_json::Value, out: &mut Vec<String>) {
        use burrow_admin_proto::serde_json::Value;
        match v {
            Value::Object(m) => {
                if let Some(Value::String(h)) = m.get("help") {
                    out.push(h.clone());
                }
                m.values().for_each(|x| helps(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| helps(x, out)),
            _ => {}
        }
    }

    #[test]
    fn the_help_under_server_fields_reads_as_plain_english() {
        let root = <burrow_config::ServerConfig as AdminSchema>::node();
        let mut all = Vec::new();
        helps(
            &burrow_admin_proto::serde_json::to_value(&root).unwrap(),
            &mut all,
        );
        assert!(all.len() > 20, "{}", all.len());
        let wrong: Vec<&String> = all
            .iter()
            .filter(|h| {
                [
                    "[",
                    "true",
                    "_secs",
                    "client: self",
                    "ip:port",
                    "host:port",
                    "CIDR",
                    "keyed by",
                ]
                .iter()
                .any(|bad| h.contains(bad))
            })
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    fn walk(key: &str, n: &Node, out: &mut Vec<(String, Node)>) {
        out.push((key.to_string(), n.clone()));
        match &n.sem {
            SemType::Object(o) => o
                .fields
                .iter()
                .for_each(|f| walk(&format!("{key}.{}", f.key), &f.node, out)),
            SemType::Map(m) => walk(&format!("{key}.*"), &m.value, out),
            SemType::RefMap(m) => walk(&format!("{key}.*"), &m.value, out),
            SemType::Optional(o) => {
                let mut inner = Vec::new();
                walk(key, &o.inner, &mut inner);
                out.extend(inner.into_iter().skip(1));
            }
            _ => {}
        }
    }

    fn wants_example(n: &Node) -> bool {
        let inner = |n: &Node| match &n.sem {
            SemType::Optional(o) => o.inner.sem.clone(),
            s => s.clone(),
        };
        match inner(n) {
            SemType::IpPort(_)
            | SemType::Target(_)
            | SemType::ServerAddress
            | SemType::Rate
            | SemType::Text(_)
            | SemType::Ip(_)
            | SemType::Host
            | SemType::HostName(_)
            | SemType::PortRange(_)
            | SemType::Size(_)
            | SemType::Path(_)
            | SemType::UrlPath => true,
            SemType::List(l) => matches!(l.item.sem, SemType::Cidr | SemType::CidrRule),
            SemType::RefMap(m) => matches!(m.value.sem, SemType::Union(_)),
            _ => false,
        }
    }

    #[test]
    fn address_and_list_fields_show_an_example() {
        let root = <burrow_config::ServerConfig as AdminSchema>::node();
        let mut all = Vec::new();
        walk("", &root, &mut all);
        let missing: Vec<String> = all
            .iter()
            .filter(|(_, n)| wants_example(n))
            .filter(|(_, n)| !n.help.contains("for example"))
            .map(|(k, n)| format!("{k}: {}", n.help))
            .collect();
        assert!(
            all.iter().filter(|(_, n)| wants_example(n)).count() > 8,
            "the walk finds the address fields"
        );
        assert!(missing.is_empty(), "{missing:#?}");
    }

    #[test]
    fn every_server_field_has_help() {
        fn fields(key: &str, n: &Node, out: &mut Vec<String>) {
            let n = match &n.sem {
                SemType::Optional(o) => &o.inner,
                _ => n,
            };
            match &n.sem {
                SemType::Object(o) => o.fields.iter().for_each(|f| {
                    let at = format!("{key}.{}", f.key);
                    let section = match &f.node.sem {
                        SemType::Optional(o) => matches!(o.inner.sem, SemType::Object(_)),
                        s => matches!(s, SemType::Object(_)),
                    };
                    if !section && f.node.help.trim().is_empty() {
                        out.push(at.clone());
                    }
                    fields(&at, &f.node, out);
                }),
                SemType::Map(m) => fields(&format!("{key}.*"), &m.value, out),
                SemType::RefMap(m) => fields(&format!("{key}.*"), &m.value, out),
                _ => {}
            }
        }
        let root = <burrow_config::ServerConfig as AdminSchema>::node();
        let mut missing = Vec::new();
        fields("", &root, &mut missing);
        assert!(missing.is_empty(), "{missing:#?}");
    }

    #[test]
    fn the_per_client_limit_names_the_client_field_that_overrides_it() {
        let root = <burrow_config::ServerConfig as AdminSchema>::node();
        let at = |p: &[&str]| {
            root.at(&p.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                .unwrap()
        };
        let limit = at(&["limits", "max_connections_per_client"]);
        let own = at(&["clients", "*", "max_connections"]);
        let label = own.label.clone();
        assert!(
            limit.help.contains(&format!("its own {label}")),
            "{} / {label}",
            limit.help
        );
    }

    #[test]
    fn help_examples_hold_for_every_server() {
        let root = <burrow_config::ServerConfig as AdminSchema>::node();
        let public = root.at(&["public_address".to_string()]).unwrap();
        assert!(
            public.help.contains("when WebSocket is on"),
            "{}",
            public.help
        );
        let clients = root
            .at(&[
                "services".to_string(),
                "*".to_string(),
                "clients".to_string(),
            ])
            .unwrap();
        assert!(
            clients.help.contains("with two or more"),
            "{}",
            clients.help
        );
    }
}
