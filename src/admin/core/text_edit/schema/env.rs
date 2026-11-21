use burrow_syntax::env::REFERENCE_PATTERN;
use serde_json::{Map, Value, json};

const ANNOTATIONS: [&str; 7] = [
    "title",
    "description",
    "default",
    "readOnly",
    "x-readonly-reason",
    "x-secret",
    "examples",
];

fn types(js: &Value) -> Vec<&str> {
    match js.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn any_string(js: &Value) -> bool {
    types(js).contains(&"string")
        && ["enum", "pattern", "const"]
            .iter()
            .all(|k| js.get(k).is_none())
}

fn wants(js: &Value) -> bool {
    if js.get("anyOf").is_some() || js.get("oneOf").is_some() {
        return false;
    }
    let t = types(js);
    let scalar = !t.contains(&"string")
        && t.iter()
            .any(|t| matches!(*t, "integer" | "number" | "boolean"));
    scalar || js.get("enum").is_some()
}

fn wrap(js: &mut Value) {
    let Value::Object(inner) = std::mem::take(js) else {
        return;
    };
    let mut inner = inner;
    let mut outer = Map::new();
    for k in ANNOTATIONS {
        if let Some(v) = inner.remove(k) {
            outer.insert(k.into(), v);
        }
    }
    outer.insert(
        "anyOf".into(),
        json!([
            Value::Object(inner),
            { "type": "string", "pattern": REFERENCE_PATTERN, "title": "An environment variable" }
        ]),
    );
    *js = Value::Object(outer);
}

pub fn references(js: &mut Value) {
    walk(js, false, 0);
}

fn walk(js: &mut Value, strings_ok: bool, depth: usize) {
    if depth > super::MAX_DEPTH {
        return;
    }
    let Some(m) = js.as_object_mut() else {
        return;
    };
    for key in ["properties", "patternProperties"] {
        if let Some(Value::Object(props)) = m.get_mut(key) {
            props.values_mut().for_each(|p| walk(p, false, depth + 1));
        }
    }
    for key in ["additionalProperties", "items"] {
        if let Some(v) = m.get_mut(key) {
            walk(v, false, depth + 1);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(list)) = m.get_mut(key) {
            let ok = key == "anyOf" && list.iter().any(any_string);
            list.iter_mut().for_each(|b| walk(b, ok, depth + 1));
        }
    }
    if !strings_ok && wants(js) {
        wrap(js);
    }
}
