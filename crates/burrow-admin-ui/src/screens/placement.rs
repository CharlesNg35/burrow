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
}
