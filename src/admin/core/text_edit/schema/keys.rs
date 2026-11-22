use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::text_edit::{DROP_IN_SCHEMA_ID, SCHEMA_ID, pointer};
use serde_json::{Value, json};

const KEYWORD: &str = "x-config-editor-keys";

fn names_of(entity: &Entity) -> Option<Value> {
    if matches!(entity, Entity::Unknown(_)) {
        return None;
    }
    let at = pointer(&burrow_config::field::entity_path(entity));
    Some(json!({ "pointer": at, "from": [SCHEMA_ID, DROP_IN_SCHEMA_ID] }))
}

fn is_text(js: &Value) -> bool {
    match js.get("type") {
        Some(Value::String(t)) => t == "string",
        Some(Value::Array(ts)) => ts.iter().any(|t| t == "string"),
        _ => false,
    }
}

fn put(js: &mut Value, keys: &Value, reserved: &[String]) {
    let Some(m) = js.as_object_mut() else {
        return;
    };
    m.insert(KEYWORD.into(), keys.clone());
    if reserved.is_empty() {
        return;
    }
    let mut examples: Vec<Value> = m
        .get("examples")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for r in reserved {
        if !examples.iter().any(|e| e == r) {
            examples.push(json!(r));
        }
    }
    m.insert("examples".into(), Value::Array(examples));
}

fn each_text(js: &mut Value, keys: &Value, reserved: &[String]) {
    if let Some(options) = js.get_mut("anyOf").and_then(Value::as_array_mut) {
        for o in options {
            each_text(o, keys, reserved);
        }
    } else if is_text(js) {
        put(js, keys, reserved);
    } else if let Some(name) = js.pointer_mut("/properties/name") {
        each_text(name, keys, reserved);
    }
}

pub(super) fn one(js: &mut Value, entity: &Entity, reserved: &[String]) {
    if let Some(keys) = names_of(entity) {
        each_text(js, &keys, reserved);
    }
}

pub(super) fn items(js: &mut Value, entity: &Entity, reserved: &[String]) {
    if let (Some(keys), Some(items)) = (names_of(entity), js.get_mut("items")) {
        each_text(items, &keys, reserved);
    }
}
