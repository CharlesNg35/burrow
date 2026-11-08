//! The editor's JSON Schema: the config model's schemars schema, inlined and marked from the admin schema tree.
//! Start at `editor_schema`; `groups` adds the admin's one-of groups as options, `env` lets `${VAR}` stand for any value.
//! It may only say less than burrow's own check: every config burrow accepts validates against it.

mod env;
mod groups;

use burrow_admin_proto::schema::{Editable, Node, Schema, SemType};
use burrow_syntax::secret_fmt::{CREDENTIAL_HEADER_ENDINGS, CREDENTIAL_HEADERS};
use serde_json::{Map, Value, json};

use crate::admin::core::values::{header_map, hidden_field};

const MAX_DEPTH: usize = 64;

pub fn editor_schema(json: Value, admin: &Schema) -> Value {
    let defs = json.get("$defs").cloned().unwrap_or(Value::Null);
    let mut root = inline(&json, &defs, 0);
    if let Some(m) = root.as_object_mut() {
        m.remove("$defs");
    }
    structure(&mut root, &admin.root, 0);
    env::references(&mut root);
    root
}

fn inline(v: &Value, defs: &Value, depth: usize) -> Value {
    match v {
        Value::Object(m) if depth < MAX_DEPTH => {
            let mut out = match m.get("$ref").and_then(Value::as_str) {
                Some(r) => {
                    let name = r.trim_start_matches("#/$defs/");
                    match inline(&defs[name], defs, depth + 1) {
                        Value::Object(d) => d,
                        _ => Map::new(),
                    }
                }
                None => Map::new(),
            };
            for (k, x) in m {
                if k != "$ref" && k != "$defs" {
                    out.insert(k.clone(), inline(x, defs, depth + 1));
                }
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| inline(x, defs, depth + 1)).collect()),
        other => other.clone(),
    }
}

fn mark(js: &mut Value, node: &Node, key: &str, is_name: bool) {
    let Some(m) = js.as_object_mut() else {
        return;
    };
    if !node.label.is_empty() {
        m.insert("title".into(), json!(node.label));
    }
    if !node.help.is_empty() {
        m.insert("description".into(), json!(node.help));
    }
    if let Some(d) = node.default.as_ref().filter(|d| !d.is_null()) {
        m.insert("default".into(), d.clone());
    }
    let reason = match &node.editable {
        Editable::No { reason } => Some(reason.clone()),
        Editable::Yes if node.locked => {
            Some("locked: change it in the config file on the server, then restart".into())
        }
        Editable::Yes => None,
    };
    if let Some(reason) = reason {
        m.insert("readOnly".into(), json!(true));
        m.insert("x-readonly-reason".into(), json!(reason));
    }
    if hidden_field(Some(node), key, is_name) {
        m.insert("x-secret".into(), json!(true));
    }
}

fn is_null(b: &Value) -> bool {
    b.get("type") == Some(&json!("null"))
}

fn leaves<'a>(js: &'a mut Value, out: &mut Vec<&'a mut Value>) {
    if js.get("anyOf").is_some_and(Value::is_array) {
        for b in js["anyOf"].as_array_mut().into_iter().flatten() {
            if !is_null(b) {
                leaves(b, out);
            }
        }
    } else {
        out.push(js);
    }
}

fn branches(js: &mut Value) -> Vec<&mut Value> {
    let mut out = Vec::new();
    leaves(js, &mut out);
    out
}

fn annotate(js: &mut Value, node: &Node, key: &str, is_name: bool, depth: usize) {
    mark(js, node, key, is_name);
    let secret = hidden_field(Some(node), key, is_name);
    for b in branches(js) {
        if secret && let Some(items) = b.get_mut("items") {
            mark_secret(items);
        }
        if depth < MAX_DEPTH {
            within(b, node, key, depth + 1);
        }
    }
}

fn mark_secret(js: &mut Value) {
    if let Some(m) = js.as_object_mut() {
        m.insert("x-secret".into(), json!(true));
    }
}

fn structure(js: &mut Value, node: &Node, depth: usize) {
    within(js, node, "", depth);
}

fn within(js: &mut Value, node: &Node, key: &str, depth: usize) {
    match &node.sem {
        SemType::Optional(o) => within(js, &o.inner, key, depth + 1),
        SemType::Object(o) => {
            if let Some(props) = js.get_mut("properties").and_then(Value::as_object_mut) {
                for f in &o.fields {
                    if let Some(p) = props.get_mut(&f.key) {
                        annotate(p, &f.node, &f.key, false, depth + 1);
                    }
                }
            }
            if js.get("properties").is_some() {
                groups::add(js, o);
            }
        }
        SemType::Map(m) => entries(js, &m.value, key, depth),
        SemType::RefMap(m) => entries(js, &m.value, key, depth),
        SemType::List(l) => {
            if let Some(items) = js.get_mut("items") {
                annotate(items, &l.item, key, false, depth + 1);
            }
        }
        SemType::Union(u) => {
            for b in &u.branches {
                if let Some(target) = object_branch(js, b) {
                    within(target, b, key, depth + 1);
                }
            }
        }
        _ => {}
    }
}

fn entries(js: &mut Value, value: &Node, key: &str, depth: usize) {
    let Some(ap) = js.get_mut("additionalProperties").filter(|a| a.is_object()) else {
        return;
    };
    annotate(ap, value, "", true, depth + 1);
    if header_map(key) {
        let mut secret = ap.clone();
        mark_secret(&mut secret);
        if let Some(m) = js.as_object_mut() {
            m.insert(
                "patternProperties".into(),
                json!({ credential_headers(): secret }),
            );
        }
    }
}

pub fn credential_headers() -> String {
    let names: Vec<&str> = CREDENTIAL_HEADERS.to_vec();
    let endings: Vec<&str> = CREDENTIAL_HEADER_ENDINGS.to_vec();
    format!("(?i)^({}|.*({}))$", names.join("|"), endings.join("|"))
}

fn fields_of(node: &Node) -> Option<Vec<&str>> {
    match &node.sem {
        SemType::Object(o) => Some(o.fields.iter().map(|f| f.key.as_str()).collect()),
        SemType::Optional(o) => fields_of(&o.inner),
        _ => None,
    }
}

fn object_branch<'a>(js: &'a mut Value, node: &Node) -> Option<&'a mut Value> {
    let keys = fields_of(node)?;
    let overlap = |b: &Value| {
        b.get("properties")
            .and_then(Value::as_object)
            .map_or(0, |p| keys.iter().filter(|k| p.contains_key(**k)).count())
    };
    let all = branches(js);
    let best = all.iter().map(|b| overlap(b)).max().filter(|n| *n > 0)?;
    all.into_iter().find(|b| overlap(b) == best)
}
