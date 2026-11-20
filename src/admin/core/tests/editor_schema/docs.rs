#![cfg(test)]

use serde_json::Value;

use super::Checks;

fn properties<'a>(js: &'a Value, at: &str, out: &mut Vec<(String, &'a Value)>) {
    let Some(m) = js.as_object() else {
        return;
    };
    for (k, v) in m
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let path = format!("{at}/{k}");
        out.push((path.clone(), v));
        properties(v, &path, out);
    }
    if let Some(v) = m.get("additionalProperties").filter(|v| v.is_object()) {
        properties(v, &format!("{at}/*"), out);
    }
    if let Some(v) = m.get("items").filter(|v| v.is_object()) {
        properties(v, &format!("{at}/[]"), out);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        for b in m.get(key).and_then(Value::as_array).into_iter().flatten() {
            properties(b, at, out);
        }
    }
}

fn all(schema: &Value) -> Vec<(String, &Value)> {
    let mut out = Vec::new();
    properties(schema, "", &mut out);
    out
}

#[test]
fn every_key_of_the_editor_schema_has_a_description() {
    let c = Checks::new();
    let mut described = std::collections::BTreeMap::<String, bool>::new();
    for (path, v) in all(c.schema()) {
        let has = v
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|d| !d.is_empty());
        *described.entry(path).or_default() |= has;
    }
    let missing: Vec<&String> = described
        .iter()
        .filter(|(_, d)| !**d)
        .map(|(p, _)| p)
        .collect();
    assert!(
        missing.is_empty(),
        "keys with no description for completion and hover: {missing:?}"
    );
}

#[test]
fn every_example_is_a_value_its_field_accepts() {
    let c = Checks::new();
    let mut seen = 0;
    for (path, v) in all(c.schema()) {
        let Some(examples) = v.get("examples").and_then(Value::as_array) else {
            continue;
        };
        let mut field = v.clone();
        if let Some(m) = field.as_object_mut() {
            m.remove("examples");
        }
        let check = jsonschema::validator_for(&field).expect("a field schema compiles");
        for e in examples {
            seen += 1;
            let errors: Vec<String> = check.iter_errors(e).map(|x| x.to_string()).collect();
            assert!(
                errors.is_empty(),
                "{path}: the example {e} is refused: {errors:?}"
            );
        }
    }
    assert!(
        seen > 50,
        "the config model gives its fields examples: {seen}"
    );
}

#[test]
fn each_choice_has_its_label_and_help_in_the_editor() {
    let c = Checks::new();
    let mode = &c.schema()["properties"]["services"]["additionalProperties"]["properties"]["mode"];
    let choices = mode["anyOf"]
        .as_array()
        .and_then(|b| b.iter().find(|b| b.get("enum").is_some()))
        .expect("mode lists its values");
    assert_eq!(
        choices["enum"],
        serde_json::json!(["failover", "balance", "least_connections", "ip_hash"])
    );
    assert_eq!(
        choices["x-enum-titles"],
        serde_json::json!(["Failover", "Balance", "Least connections", "IP hash"])
    );
    let docs = choices["x-enum-descriptions"]
        .as_array()
        .expect("each value has its help");
    assert_eq!(docs.len(), 4);
    assert!(
        docs.iter()
            .all(|d| d.as_str().is_some_and(|d| !d.is_empty())),
        "{docs:?}"
    );
}
