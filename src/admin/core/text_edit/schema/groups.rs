use burrow_admin_proto::schema::{ObjectC, OneOf};
use serde_json::{Map, Value, json};

fn given(keys: &[String]) -> Value {
    let props: Map<String, Value> = keys
        .iter()
        .map(|k| (k.clone(), json!({ "not": { "type": "null" } })))
        .collect();
    json!({ "required": keys, "properties": props })
}

fn label_of(o: &ObjectC, key: &str) -> Option<(String, String)> {
    let f = o.fields.iter().find(|f| f.key == key)?;
    Some((f.node.label.clone(), f.node.help.clone()))
}

fn option(o: &ObjectC, choice: &[String]) -> Value {
    let mut b = given(choice);
    let (titles, helps): (Vec<String>, Vec<String>) = choice
        .iter()
        .filter_map(|k| label_of(o, k))
        .filter(|(t, _)| !t.is_empty())
        .unzip();
    if let Some(m) = b.as_object_mut() {
        if !titles.is_empty() {
            m.insert("title".into(), json!(titles.join(" and ")));
        }
        if let Some(h) = helps.into_iter().find(|h| !h.is_empty()) {
            m.insert("description".into(), json!(h));
        }
    }
    b
}

fn group(o: &ObjectC, g: &OneOf) -> Value {
    let mut options: Vec<Value> = g.choices.iter().map(|c| option(o, c)).collect();
    if g.required {
        return Value::Array(options);
    }
    let any: Vec<Value> = g
        .choices
        .iter()
        .flatten()
        .map(|k| given(std::slice::from_ref(k)))
        .collect();
    options.push(json!({
        "title": format!("No {}", g.label.to_lowercase()),
        "description": g.help,
        "not": { "anyOf": any },
    }));
    Value::Array(options)
}

pub fn add(js: &mut Value, o: &ObjectC) {
    let Some(m) = js.as_object_mut() else {
        return;
    };
    match o.one_of.as_slice() {
        [] => {}
        [one] => {
            m.insert("oneOf".into(), group(o, one));
        }
        many => {
            let all: Vec<Value> = many
                .iter()
                .map(|g| json!({ "oneOf": group(o, g) }))
                .collect();
            m.insert("allOf".into(), Value::Array(all));
        }
    }
}
