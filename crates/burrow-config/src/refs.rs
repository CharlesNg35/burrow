use std::collections::BTreeSet;

use burrow_admin_proto::Path;
use burrow_admin_proto::serde_json::{Map, Value};
use burrow_admin_proto::value::get;

use burrow_admin_proto::schema::Entity;

use crate::field::{Entry, entity_at};
use crate::model::{ClientEntry, ServerConfig, ServiceEntry, VisitorEntry, WebSite};

pub fn rename_refs(root: &mut Value, entry: &[String], to: &str) -> Vec<Path> {
    let Some((from, parent)) = entry.split_last() else {
        return Vec::new();
    };
    match entity_at(parent) {
        Some(Entity::Login) => {
            rename_login(root, from, to);
            Vec::new()
        }
        Some(e) => rename_in_services(root, e.section(), from, to),
        None => Vec::new(),
    }
}

fn rename_login(root: &mut Value, from: &str, to: &str) {
    let Some(services) = ServerConfig::SERVICES
        .get_mut(root)
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for svc in services.values_mut() {
        if let Some(v) = ServiceEntry::WEB
            .then(WebSite::FORWARD_AUTH)
            .get_mut(svc)
            .filter(|v| v.as_str().map(str::trim) == Some(from))
        {
            *v = Value::String(to.to_string());
        }
    }
}

fn rename_in_services(root: &mut Value, section: &str, from: &str, to: &str) -> Vec<Path> {
    let mut keys = Vec::new();
    let fields = if section == ClientEntry::section() {
        vec![ServiceEntry::CLIENT.key(), ServiceEntry::CLIENTS.key()]
    } else if section == VisitorEntry::section() {
        vec![ServiceEntry::VISITORS.key()]
    } else {
        Vec::new()
    };
    if let Some(services) = ServerConfig::SERVICES
        .get_mut(root)
        .and_then(Value::as_object_mut)
    {
        for svc in services.values_mut() {
            let Some(svc) = svc.as_object_mut() else {
                continue;
            };
            for f in &fields {
                match svc.get_mut(*f) {
                    Some(Value::Array(items)) => items.iter_mut().for_each(|v| {
                        if v.as_str() == Some(from) {
                            *v = Value::String(to.to_string());
                        }
                    }),
                    Some(v) if v.as_str() == Some(from) => *v = Value::String(to.to_string()),
                    _ => {}
                }
            }
        }
    }
    if section == ClientEntry::section()
        && let Some(visitors) = ServerConfig::VISITORS
            .get_mut(root)
            .and_then(Value::as_object_mut)
    {
        for (name, v) in visitors.iter_mut() {
            if let Some(reach) = VisitorEntry::REACH
                .get_mut(v)
                .and_then(Value::as_object_mut)
                && reach.contains_key(from)
            {
                keys.push(VisitorEntry::REACH.entry_of(name, from));
                let renamed: Vec<(String, Value)> = std::mem::take(reach)
                    .into_iter()
                    .map(|(k, x)| (if k == from { to.to_string() } else { k }, x))
                    .collect();
                reach.extend(renamed);
            }
        }
    }
    keys
}

struct Refs {
    rest: Value,
    names: Vec<(&'static str, BTreeSet<String>)>,
    order: Vec<Value>,
}

fn names_in(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn refs(section: &str, v: &Value, shared: &BTreeSet<String>) -> Option<Refs> {
    let mut rest = v.as_object().cloned().unwrap_or_default();
    if section == ServiceEntry::section() {
        let mut clients = names_in(rest.remove(ServiceEntry::CLIENT.key()).as_ref());
        clients.extend(names_in(rest.remove(ServiceEntry::CLIENTS.key()).as_ref()));
        let visitors = names_in(rest.remove(ServiceEntry::VISITORS.key()).as_ref());
        let order = clients
            .iter()
            .filter(|c| shared.contains(*c))
            .map(|c| Value::String(c.clone()))
            .collect();
        return Some(Refs {
            rest: Value::Object(rest),
            names: vec![
                (ClientEntry::section(), clients.into_iter().collect()),
                (VisitorEntry::section(), visitors.into_iter().collect()),
            ],
            order,
        });
    }
    if section == VisitorEntry::section() {
        let reach = rest
            .remove(VisitorEntry::REACH.key())
            .and_then(|r| r.as_object().cloned())
            .unwrap_or_default();
        let kept: Map<String, Value> = reach
            .iter()
            .filter(|(k, _)| shared.contains(*k))
            .map(|(k, x)| (k.clone(), x.clone()))
            .collect();
        return Some(Refs {
            rest: Value::Object(rest),
            names: vec![(ClientEntry::section(), reach.keys().cloned().collect())],
            order: vec![Value::Object(kept)],
        });
    }
    None
}

fn event(
    base: &Value,
    draft: &Value,
    renames: &[(Path, String)],
    section: &str,
    name: &str,
) -> Option<Path> {
    for (from, to) in renames {
        if from.first().map(String::as_str) == Some(section)
            && (from.get(1).map(String::as_str) == Some(name) || to == name)
        {
            return Some(from.clone());
        }
    }
    let path = vec![section.to_string(), name.to_string()];
    (get(base, &path).is_some() != get(draft, &path).is_some()).then_some(path)
}

pub fn part_of(
    base: &Value,
    draft: &Value,
    entity: &[String],
    renames: &[(Path, String)],
) -> Option<Path> {
    let [section, _] = entity else {
        return None;
    };
    let (old, new) = (get(base, entity)?, get(draft, entity)?);
    let keys = |doc: &Value, sec: &str| -> BTreeSet<String> {
        get(doc, &[sec.to_string()])
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    };
    let mut shared = BTreeSet::new();
    for sec in [ClientEntry::section(), VisitorEntry::section()] {
        shared.extend(keys(base, sec).intersection(&keys(draft, sec)).cloned());
    }
    let (a, b) = (refs(section, old, &shared)?, refs(section, new, &shared)?);
    if a.rest != b.rest || a.order != b.order {
        return None;
    }
    let mut found: Option<Path> = None;
    let mut any = false;
    for ((sec, before), (_, after)) in a.names.iter().zip(b.names.iter()) {
        for name in before.symmetric_difference(after) {
            any = true;
            let id = event(base, draft, renames, sec, name)?;
            match &found {
                Some(f) if *f != id => return None,
                _ => found = Some(id),
            }
        }
    }
    if any { found } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn a_renamed_client_keeps_its_place_in_every_reference() {
        let mut doc = json!({
            "services": {
                "web": {"client": "home", "target": 80},
                "db": {"clients": ["office", "home"], "target": 5432},
                "ssh": {"visitors": ["home"], "target": 22}
            },
            "visitors": {"lap": {"reach": {"home": "any", "office": "any", "lab": "any"}}}
        });
        let keys = rename_refs(&mut doc, &p("clients", "home"), "home2");
        assert_eq!(keys, [VisitorEntry::REACH.entry_of("lap", "home")]);
        assert_eq!(doc["services"]["web"]["client"], "home2");
        assert_eq!(doc["services"]["db"]["clients"], json!(["office", "home2"]));
        assert_eq!(doc["services"]["ssh"]["visitors"], json!(["home"]));
        let keys: Vec<&String> = doc["visitors"]["lap"]["reach"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(keys, ["home2", "office", "lab"]);
        rename_refs(&mut doc, &p("visitors", "home"), "guest");
        assert_eq!(doc["services"]["ssh"]["visitors"], json!(["guest"]));
        assert_eq!(doc["services"]["web"]["client"], "home2");
    }

    fn p(a: &str, b: &str) -> Path {
        vec![a.to_string(), b.to_string()]
    }

    #[test]
    fn a_renamed_login_service_is_renamed_on_every_site_that_names_it() {
        let mut doc = json!({
            "web": {"forward_auth": {"sso": {"service": "authelia", "path": "/a"}}},
            "services": {
                "wiki": {"client": "home", "target": 80, "web": {"hosts": ["w.example.com"], "forward_auth": "sso"}},
                "dav": {"client": "home", "target": 81, "web": {"hosts": ["d.example.com"], "forward_auth": true}},
                "blog": {"client": "home", "target": 82, "web": "b.example.com"},
                "sso": {"client": "home", "target": 9091}
            }
        });
        let at = vec![
            "web".to_string(),
            "forward_auth".to_string(),
            "sso".to_string(),
        ];
        assert!(rename_refs(&mut doc, &at, "authelia").is_empty());
        assert_eq!(doc["services"]["wiki"]["web"]["forward_auth"], "authelia");
        assert_eq!(doc["services"]["dav"]["web"]["forward_auth"], true);
        assert_eq!(doc["services"]["blog"]["web"], "b.example.com");
        assert_eq!(
            doc["services"]["sso"]["client"], "home",
            "a service of the same name stays"
        );
        assert!(rename_refs(&mut doc, &p("services", "sso"), "x").is_empty());
        assert_eq!(doc["services"]["wiki"]["web"]["forward_auth"], "authelia");
    }

    #[test]
    fn a_service_that_only_gains_a_new_client_is_part_of_that_client() {
        let base = json!({"clients": {"home": {}}, "services": {
            "web": {"client": "home", "target": 80},
            "db": {"clients": ["home"], "target": 5432},
            "ssh": {"client": "home", "target": 22}
        }});
        let draft = json!({"clients": {"home": {}, "example": {}}, "services": {
            "web": {"clients": ["home", "example"], "target": 80},
            "db": {"clients": ["home", "example"], "target": 5432},
            "ssh": {"clients": ["home", "example"], "target": 2222}
        }});
        let c = p("clients", "example");
        assert_eq!(
            part_of(&base, &draft, &p("services", "web"), &[]),
            Some(c.clone())
        );
        assert_eq!(part_of(&base, &draft, &p("services", "db"), &[]), Some(c));
        assert_eq!(
            part_of(&base, &draft, &p("services", "ssh"), &[]),
            None,
            "the target changed too"
        );
        assert_eq!(part_of(&base, &draft, &p("clients", "example"), &[]), None);
    }

    #[test]
    fn removed_and_renamed_entities_take_their_references_with_them() {
        let base = json!({"clients": {"home": {}, "office": {}}, "visitors": {"lap": {"reach": {"office": "any"}}, "bob": {}}, "services": {
            "web": {"clients": ["home", "office"], "visitors": ["bob"], "target": 80}
        }});
        let removed = json!({"clients": {"home": {}}, "visitors": {"lap": {}, "bob": {}}, "services": {
            "web": {"clients": ["home"], "visitors": ["bob"], "target": 80}
        }});
        let office = p("clients", "office");
        assert_eq!(
            part_of(&base, &removed, &p("services", "web"), &[]),
            Some(office.clone())
        );
        assert_eq!(
            part_of(&base, &removed, &p("visitors", "lap"), &[]),
            Some(office.clone())
        );
        let renamed = json!({"clients": {"home": {}, "branch": {}}, "visitors": {"lap": {"reach": {"branch": "any"}}, "bob": {}}, "services": {
            "web": {"clients": ["home", "branch"], "visitors": ["bob"], "target": 80}
        }});
        let r = vec![(office.clone(), "branch".to_string())];
        assert_eq!(
            part_of(&base, &renamed, &p("services", "web"), &r),
            Some(office.clone())
        );
        assert_eq!(
            part_of(&base, &renamed, &p("visitors", "lap"), &r),
            Some(office)
        );
    }

    #[test]
    fn a_reordered_list_or_two_new_entities_stand_on_their_own() {
        let base = json!({"clients": {"a": {}, "b": {}}, "services": {"web": {"clients": ["a", "b"], "target": 80}}});
        let order = json!({"clients": {"a": {}, "b": {}}, "services": {"web": {"clients": ["b", "a"], "target": 80}}});
        assert_eq!(part_of(&base, &order, &p("services", "web"), &[]), None);
        let two = json!({"clients": {"a": {}, "b": {}, "c": {}, "d": {}}, "services": {"web": {"clients": ["a", "b", "c", "d"], "target": 80}}});
        assert_eq!(part_of(&base, &two, &p("services", "web"), &[]), None);
        let existing = json!({"clients": {"a": {}, "b": {}}, "services": {"web": {"clients": ["a"], "target": 80}}});
        assert_eq!(
            part_of(&base, &existing, &p("services", "web"), &[]),
            None,
            "b still exists"
        );
    }
}
