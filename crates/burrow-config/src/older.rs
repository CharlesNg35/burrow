use burrow_admin_proto::Path;
use burrow_admin_proto::older::{V0_1_2_ENTITIES, V0_1_2_TAGS};
use burrow_admin_proto::schema::{Entity, Node, SemType};
use burrow_admin_proto::serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    pub path: Path,
    pub label: String,
}

fn known_entity(e: &Entity) -> bool {
    V0_1_2_ENTITIES.contains(&e.as_str())
}

fn new_to_v0_1_2(sem: &SemType) -> bool {
    !V0_1_2_TAGS.contains(&sem.tag())
        || match sem {
            SemType::Ref(r) => !known_entity(&r.entity),
            SemType::RefList(r) => !known_entity(&r.entity),
            SemType::Optional(o) => new_to_v0_1_2(&o.inner.sem),
            _ => false,
        }
}

fn fits(sem: &SemType, v: &Value) -> bool {
    match sem {
        SemType::Optional(o) => fits(&o.inner.sem, v),
        SemType::Bool => v.is_boolean(),
        SemType::Object(_) | SemType::Map(_) | SemType::RefMap(_) => v.is_object(),
        SemType::List(_) | SemType::RefList(_) | SemType::RefEntries(_) => v.is_array(),
        SemType::Union(u) => u.branches.iter().any(|b| fits(&b.sem, v)),
        _ => v.is_string() || v.is_number(),
    }
}

pub fn v0_1_2_cannot_edit(root: &Node, values: &Value) -> Vec<Spot> {
    let mut out = Vec::new();
    let top = Spot {
        path: Vec::new(),
        label: String::new(),
    };
    walk(root, values, &mut Vec::new(), &top, &mut out);
    out
}

fn note(field: &Spot, out: &mut Vec<Spot>) {
    if !out.iter().any(|s| s.path == field.path) {
        out.push(field.clone());
    }
}

fn walk(node: &Node, v: &Value, path: &mut Path, field: &Spot, out: &mut Vec<Spot>) {
    if v.is_null() {
        return;
    }
    match &node.sem {
        SemType::Optional(o) => walk(&o.inner, v, path, field, out),
        sem if new_to_v0_1_2(sem) => note(field, out),
        SemType::Object(o) => {
            for f in &o.fields {
                let Some(x) = v.get(&f.key) else { continue };
                path.push(f.key.clone());
                let label = if field.label.is_empty() {
                    f.node.label.clone()
                } else {
                    field.label.clone()
                };
                let here = Spot {
                    path: path.clone(),
                    label,
                };
                walk(&f.node, x, path, &here, out);
                path.pop();
            }
        }
        SemType::Map(m) => each_entry(&m.value, v, path, field, out),
        SemType::RefMap(m) => each_entry(&m.value, v, path, field, out),
        SemType::List(l) => {
            for (i, x) in v.as_array().into_iter().flatten().enumerate() {
                path.push(i.to_string());
                walk(&l.item, x, path, field, out);
                path.pop();
            }
        }
        SemType::Union(u) => match u.branches.iter().find(|b| fits(&b.sem, v)) {
            Some(b) => walk(b, v, path, field, out),
            None if u.branches.iter().any(|b| new_to_v0_1_2(&b.sem)) => note(field, out),
            None => {}
        },
        _ => {}
    }
}

fn each_entry(value: &Node, v: &Value, path: &mut Path, field: &Spot, out: &mut Vec<Spot>) {
    let entity = Spot {
        path: path.clone(),
        label: String::new(),
    };
    let field = if path.len() == 1 { &entity } else { field };
    for (k, x) in v.as_object().into_iter().flatten() {
        path.push(k.clone());
        walk(value, x, path, field, out);
        path.pop();
    }
}

fn spot_words(s: &Spot) -> String {
    let label = match (s.label.is_empty(), s.path.last()) {
        (false, _) | (true, None) => s.label.clone(),
        (true, Some(k)) => k.clone(),
    };
    let entity = [Entity::Client, Entity::Visitor, Entity::Service]
        .into_iter()
        .find(|e| s.path.first().is_some_and(|f| f == e.section()));
    match (entity, s.path.get(1)) {
        (Some(e), Some(name)) => format!("{label} of {} {name}", e.as_str()),
        _ => format!("{label} in Settings"),
    }
}

pub fn older_admin(server: &str, spots: &[Spot]) -> (String, String) {
    let mut words: Vec<String> = Vec::new();
    for w in spots.iter().map(spot_words) {
        if !words.contains(&w) {
            words.push(w);
        }
    }
    (
        format!(
            "This burrow admin is older than the server (v{server}) and cannot edit some settings here: {}",
            words.join(", ")
        ),
        "Update burrow on this machine".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServerConfig;
    use crate::admin_schema::AdminSchema;
    use burrow_admin_proto::serde_json::json;

    fn spots(doc: Value) -> Vec<String> {
        v0_1_2_cannot_edit(&ServerConfig::node(), &doc)
            .into_iter()
            .map(|s| format!("{} = {}", s.path.join("."), s.label))
            .collect()
    }

    #[test]
    fn a_config_v0_1_2_could_write_has_nothing_it_cannot_edit() {
        let doc = json!({
            "listen": "127.0.0.1:7835",
            "clients": {"home": {"secret": "x", "max_connections": 100}},
            "visitors": {"ops": {"secret": "y", "admin": "write", "admin_from": ["127.0.0.1/32"]},
                         "laptop": {"secret": "z", "reach": {"home": ["127.0.0.1/32"]}}},
            "services": {
                "web": {"client": "home", "bind": "127.0.0.1:9000", "target": 3000},
                "ssh": {"client": "home", "target": 22, "visitors": ["laptop"]}
            }
        });
        assert_eq!(spots(doc), Vec::<String>::new());
    }

    #[test]
    fn client_entries_a_website_and_a_named_login_service_are_new_to_v0_1_2() {
        let doc = json!({
            "listen": 7835,
            "web": {"https": 443, "forward_auth": {"authelia": {"service": "auth", "path": "/api/verify"}}},
            "clients": {"a": {"secret": "x"}, "b": {"secret": "y"}},
            "services": {
                "auth": {"client": "a", "target": 9091, "bind": 9091},
                "app": {"clients": ["a", {"name": "b", "target": "10.0.0.2:80", "weight": 2}], "target": 80, "bind": 8080},
                "nas": {"client": "a", "target": 5000, "web": {"hosts": ["nas.example.com"], "forward_auth": "authelia"}}
            }
        });
        let got = spots(doc);
        assert_eq!(
            got,
            [
                "services.app.clients = Clients",
                "services.nas.web.hosts = Names",
                "services.nas.web.forward_auth = Names"
            ],
            "a nested field is named by the entity field holding it (the website's Names), and services.auth is plain"
        );
    }

    #[test]
    fn a_tag_or_entity_v0_1_2_knows_is_editable_and_a_new_one_is_not() {
        assert!(!new_to_v0_1_2(&SemType::Bool));
        let login = SemType::Ref(burrow_admin_proto::schema::RefC {
            entity: Entity::Login,
            reserved: Vec::new(),
        });
        assert!(new_to_v0_1_2(&login));
        let client = SemType::Ref(burrow_admin_proto::schema::RefC {
            entity: Entity::Client,
            reserved: Vec::new(),
        });
        assert!(!new_to_v0_1_2(&client));
        let entries = SemType::RefEntries(burrow_admin_proto::schema::RefEntriesC::new(
            Entity::Client,
            Vec::new(),
        ));
        assert!(new_to_v0_1_2(&entries));
    }

    #[test]
    fn an_older_admin_is_told_in_plain_words_what_it_cannot_edit() {
        let p = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let spots = [
            Spot {
                path: p(&["services", "app", "clients"]),
                label: "Clients".into(),
            },
            Spot {
                path: p(&["services", "nas", "web", "forward_auth"]),
                label: "Login service".into(),
            },
            Spot {
                path: p(&["web", "certificates", "0", "cert"]),
                label: "Certificate file".into(),
            },
            Spot {
                path: p(&["web", "certificates", "1", "cert"]),
                label: "Certificate file".into(),
            },
        ];
        let (message, help) = older_admin("0.2.0", &spots);
        assert_eq!(
            message,
            "This burrow admin is older than the server (v0.2.0) and cannot edit some settings here: Clients of service app, Login service of service nas, Certificate file in Settings"
        );
        assert_eq!(help, "Update burrow on this machine");
    }
}
