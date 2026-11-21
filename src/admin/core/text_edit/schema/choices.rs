use burrow_admin_proto::schema::EnumC;
use serde_json::{Value, json};

pub(super) fn add(js: &mut Value, e: &EnumC) {
    let Some(values) = js.get("enum").and_then(Value::as_array) else {
        return;
    };
    let option = |v: &Value| {
        v.as_str()
            .and_then(|v| e.options.iter().find(|o| o.value == v))
    };
    let titles: Vec<String> = values
        .iter()
        .map(|v| option(v).map_or_else(String::new, |o| o.label.clone()))
        .collect();
    let docs: Vec<String> = values
        .iter()
        .map(|v| option(v).map_or_else(String::new, |o| o.help.clone()))
        .collect();
    let named = values
        .iter()
        .zip(&titles)
        .any(|(v, t)| !t.is_empty() && v.as_str() != Some(t.as_str()));
    let Some(m) = js.as_object_mut() else {
        return;
    };
    if named {
        m.insert("x-enum-titles".into(), json!(titles));
    }
    if docs.iter().any(|d| !d.is_empty()) {
        m.insert("x-enum-descriptions".into(), json!(docs));
    }
}
