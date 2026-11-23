use std::collections::{BTreeMap, BTreeSet};

use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{EntityInfo, Node, Schema, SemType};
use burrow_admin_proto::serde_json::{Map, Value};

use crate::relations::entity_problems;

pub type Errors = BTreeMap<Path, String>;

fn item_node<'a>(schema: &'a Schema, info: &EntityInfo) -> Option<&'a Node> {
    let node = schema.root.at(&info.path)?;
    match &crate::tree::inner(node).sem {
        SemType::Map(m) => Some(&m.value),
        _ => None,
    }
}

fn items<'a>(doc: &'a Value, info: &EntityInfo) -> Option<&'a Map<String, Value>> {
    burrow_admin_proto::value::get(doc, &info.path).and_then(Value::as_object)
}

fn add_item(out: &mut Errors, node: &Node, at: &[String], name: &str, value: &Value) {
    for (key, msg) in entity_problems(node, value) {
        let mut p = at.to_vec();
        p.extend([name.to_string(), key]);
        out.entry(p).or_insert(msg);
    }
}

pub fn entity_errors(schema: &Schema, doc: &Value) -> Errors {
    let mut out = BTreeMap::new();
    for info in &schema.entities {
        let (Some(node), Some(items)) = (item_node(schema, info), items(doc, info)) else {
            continue;
        };
        for (name, value) in items {
            add_item(&mut out, node, &info.path, name, value);
        }
    }
    out
}

fn forget(out: &mut Errors, prefix: &[String]) {
    let gone: Vec<Path> = out
        .range(prefix.to_vec()..)
        .take_while(|(p, _)| p.starts_with(prefix))
        .map(|(p, _)| p.clone())
        .collect();
    for p in gone {
        out.remove(&p);
    }
}

enum Touched {
    All,
    Items(BTreeSet<String>),
}

fn touched(info: &EntityInfo, changed: &[Path]) -> Option<Touched> {
    let mut names = BTreeSet::new();
    for p in changed {
        if info.path.starts_with(p) {
            return Some(Touched::All);
        }
        if let Some(name) = p
            .strip_prefix(info.path.as_slice())
            .and_then(<[String]>::first)
        {
            names.insert(name.clone());
        }
    }
    (!names.is_empty()).then_some(Touched::Items(names))
}

pub fn refresh(schema: &Schema, doc: &Value, out: &mut Errors, changed: &[Path]) {
    for info in &schema.entities {
        let Some(node) = item_node(schema, info) else {
            continue;
        };
        let all = items(doc, info);
        match touched(info, changed) {
            None => {}
            Some(Touched::All) => {
                forget(out, &info.path);
                for (name, value) in all.into_iter().flatten() {
                    add_item(out, node, &info.path, name, value);
                }
            }
            Some(Touched::Items(names)) => {
                for name in names {
                    let mut at = info.path.clone();
                    at.push(name.clone());
                    forget(out, &at);
                    if let Some(value) = all.and_then(|m| m.get(&name)) {
                        add_item(out, node, &info.path, &name, value);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{doc, schema};
    use burrow_admin_proto::serde_json::json;

    fn p(s: &[&str]) -> Path {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_refresh_after_edits_matches_a_full_check() {
        let schema = schema();
        let mut d = doc();
        let mut errs = entity_errors(&schema, &d);
        let edits: Vec<(Path, Option<Value>)> = vec![
            (p(&["services", "web", "client"]), None),
            (p(&["services", "web", "clients"]), Some(json!(["home"]))),
            (p(&["services", "both", "target"]), Some(json!("3001-3003"))),
            (p(&["services", "new"]), Some(json!({"target": 9}))),
            (p(&["services", "dns"]), None),
            (p(&["services", "web", "client"]), Some(json!("home"))),
            (p(&["services"]), Some(json!({"x": {"target": 1}}))),
        ];
        for (path, v) in edits {
            burrow_admin_proto::value::put(&mut d, &path, v);
            refresh(&schema, &d, &mut errs, std::slice::from_ref(&path));
            assert_eq!(errs, entity_errors(&schema, &d), "after {path:?}");
        }
        assert!(!errs.is_empty(), "the edits made problems to compare");
    }

    #[test]
    fn a_change_outside_entities_keeps_the_result() {
        let schema = schema();
        let d = doc();
        let mut errs = entity_errors(&schema, &d);
        errs.insert(p(&["services", "web", "marker"]), "kept".into());
        refresh(&schema, &d, &mut errs, &[p(&["listen"])]);
        assert!(errs.contains_key(&p(&["services", "web", "marker"])));
    }
}
