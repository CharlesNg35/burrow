use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Entity, EntityInfo, Node, Schema, SemType};
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::{self, as_env};

#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    Value(Value),
    Unset,
    Unknown,
}

fn resolve_path(base: &[String], rel: &str) -> Path {
    let (mut out, rest) = match rel.strip_prefix('/') {
        Some(abs) => (Vec::new(), abs),
        None => (base.to_vec(), rel),
    };
    out.extend(
        rest.split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    );
    out
}

pub fn lookup(schema: &Node, doc: &Value, base: &[String], rel: &str) -> Lookup {
    let path = resolve_path(base, rel);
    match value::get(doc, &path) {
        Some(v) if !v.is_null() => match as_env(v) {
            Some(e) => e.resolved.map_or(Lookup::Unknown, |r| {
                Lookup::Value(
                    r.parse::<i64>()
                        .map(Value::from)
                        .unwrap_or(Value::String(r)),
                )
            }),
            None => Lookup::Value(v.clone()),
        },
        _ => match schema.at(&path) {
            None => Lookup::Unknown,
            Some(n) => n.default.clone().map_or(Lookup::Unset, Lookup::Value),
        },
    }
}

fn entity_info<'a>(schema: &'a Schema, entity: &Entity) -> Option<&'a EntityInfo> {
    schema.entities.iter().find(|e| &e.entity == entity)
}

pub fn entity_sections(schema: &Schema) -> Vec<String> {
    schema
        .entities
        .iter()
        .filter_map(|e| match e.path.as_slice() {
            [section] => Some(section.clone()),
            _ => None,
        })
        .collect()
}

pub fn names(doc: &Value, section: &[String]) -> Vec<String> {
    value::get(doc, section)
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

pub fn entity_names(schema: &Schema, doc: &Value, entity: &Entity) -> Vec<String> {
    entity_info(schema, entity)
        .map(|e| names(doc, &e.path))
        .unwrap_or_default()
}

pub fn inner(node: &Node) -> &Node {
    match &node.sem {
        SemType::Optional(o) => inner(&o.inner),
        _ => node,
    }
}

pub fn fields(node: &Node) -> &[burrow_admin_proto::schema::Field] {
    match &inner(node).sem {
        SemType::Object(o) => &o.fields,
        _ => &[],
    }
}

fn one_of(node: &Node) -> Option<&str> {
    node.relations.iter().find_map(|r| match r {
        burrow_admin_proto::schema::Relation::OneOf { group } => Some(group.as_str()),
        _ => None,
    })
}

fn pair(object: &Node) -> Option<(&str, &str)> {
    let all = fields(object);
    all.iter().find_map(|list| {
        let entity = match &inner(&list.node).sem {
            SemType::RefList(l) => &l.entity,
            SemType::RefEntries(l) => &l.entity,
            _ => return None,
        };
        let group = one_of(&list.node)?;
        all.iter()
            .find_map(|single| match &inner(&single.node).sem {
                SemType::Ref(r) if &r.entity == entity && one_of(&single.node) == Some(group) => {
                    Some((single.key.as_str(), list.key.as_str()))
                }
                _ => None,
            })
    })
}

pub fn merged_single<'a>(object: &'a Node, list_key: &str) -> Option<&'a str> {
    pair(object).filter(|(_, l)| *l == list_key).map(|(s, _)| s)
}

pub fn merged_into<'a>(object: &'a Node, single_key: &str) -> Option<&'a str> {
    pair(object)
        .filter(|(s, _)| *s == single_key)
        .map(|(_, l)| l)
}

pub fn holds(doc: &Value, pattern: &[String]) -> bool {
    let Some((first, rest)) = pattern.split_first() else {
        return !doc.is_null();
    };
    match doc {
        Value::Object(m) if first == burrow_admin_proto::schema::ANY => {
            m.values().any(|v| holds(v, rest))
        }
        Value::Array(a) if first == burrow_admin_proto::schema::ANY => {
            a.iter().any(|v| holds(v, rest))
        }
        Value::Object(m) => m.get(first).is_some_and(|v| holds(v, rest)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{doc, schema};

    fn p(s: &[&str]) -> Path {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_single_reference_merges_into_its_list_through_one_of() {
        let s = schema();
        let svc = s.root.at(&p(&["services", "*"])).unwrap();
        assert_eq!(merged_single(svc, "clients"), Some("client"));
        assert_eq!(merged_into(svc, "client"), Some("clients"));
        assert_eq!(merged_single(svc, "visitors"), None);
        assert_eq!(merged_into(svc, "target"), None);
        let visitor = s.root.at(&p(&["visitors", "*"])).unwrap();
        assert_eq!(merged_single(visitor, "reach"), None);
    }

    #[test]
    fn paths_resolve_relative_to_the_entity_or_from_the_root() {
        let base = p(&["services", "web"]);
        assert_eq!(
            resolve_path(&base, "protocol"),
            p(&["services", "web", "protocol"])
        );
        assert_eq!(resolve_path(&base, "/limits/x"), p(&["limits", "x"]));
    }

    #[test]
    fn lookups_fall_back_to_schema_defaults() {
        let s = schema();
        let d = doc();
        let web = p(&["services", "web"]);
        assert_eq!(
            lookup(&s.root, &d, &web, "protocol"),
            Lookup::Value("tcp".into())
        );
        assert_eq!(
            lookup(&s.root, &d, &web, "client"),
            Lookup::Value("home".into())
        );
        assert_eq!(lookup(&s.root, &d, &web, "bandwidth"), Lookup::Unset);
        assert_eq!(lookup(&s.root, &d, &web, "future_field"), Lookup::Unknown);
    }

    #[test]
    fn entity_names_come_from_the_entity_sections() {
        let s = schema();
        let d = doc();
        assert_eq!(
            entity_names(&s, &d, &Entity::Client),
            vec!["home", "office"]
        );
        assert!(entity_sections(&s).contains(&"clients".to_string()));
        assert!(
            !entity_sections(&s).contains(&"web".to_string()),
            "the login services live inside the web settings"
        );
    }
}
