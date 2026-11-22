#![cfg(test)]

use std::collections::BTreeSet;

use burrow_admin_proto::msg::AdminLevel;
use burrow_admin_proto::text_edit::{DROP_IN_SCHEMA_ID, SCHEMA_ID};
use serde_json::Value;

use super::super::text_edit::edit_text;
use super::super::{core_running, session, world};
use super::Checks;

fn named(js: &Value, at: &str, out: &mut BTreeSet<(String, String)>) {
    let Some(m) = js.as_object() else {
        return;
    };
    if let Some(k) = m.get("x-config-editor-keys") {
        assert_eq!(
            k["from"],
            serde_json::json!([SCHEMA_ID, DROP_IN_SCHEMA_ID]),
            "{at}"
        );
        out.insert((at.to_string(), k["pointer"].as_str().unwrap().to_string()));
    }
    for (k, v) in m
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        named(v, &format!("{at}/{k}"), out);
    }
    if let Some(v) = m.get("additionalProperties") {
        named(v, &format!("{at}/*"), out);
    }
    if let Some(v) = m.get("items") {
        named(v, &format!("{at}/[]"), out);
    }
    for key in ["anyOf", "oneOf"] {
        for b in m.get(key).and_then(Value::as_array).into_iter().flatten() {
            named(b, at, out);
        }
    }
}

fn examples_at<'a>(js: &'a Value, path: &[&str], out: &mut Vec<&'a Value>) {
    let Some((first, rest)) = path.split_first() else {
        out.extend(
            js.get("examples")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        );
        return;
    };
    let next = match *first {
        "*" => js.get("additionalProperties"),
        "[]" => js.get("items"),
        k => js.get("properties").and_then(|p| p.get(k)),
    };
    if let Some(n) = next {
        examples_at(n, rest, out);
    }
    for b in js
        .get("anyOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        examples_at(b, path, out);
    }
}

#[test]
fn a_field_naming_a_client_visitor_service_or_login_offers_the_names_defined() {
    let c = Checks::new();
    let mut found = BTreeSet::new();
    named(c.schema(), "", &mut found);
    let want = |p: &str, at: &str| (p.to_string(), at.to_string());
    let mut expected: BTreeSet<_> = [
        want("/services/*/client", "/clients"),
        want("/services/*/clients/[]", "/clients"),
        want("/services/*/clients/[]/name", "/clients"),
        want("/services/*/visitors/[]", "/visitors"),
    ]
    .into();
    if crate::base::build::WEB_PROXY {
        expected.insert(want("/web/forward_auth/*/service", "/services"));
        expected.insert(want("/services/*/web/forward_auth", "/web/forward_auth"));
    }
    assert_eq!(found, expected);
    let mut self_ = Vec::new();
    examples_at(c.schema(), &["services", "*", "client"], &mut self_);
    assert!(
        self_.iter().any(|e| *e == "self"),
        "self is offered as a word, not a name: {self_:?}"
    );
    let w = world();
    let core = core_running(&w.text, &w.text);
    let t = edit_text(&core, &session(&w, AdminLevel::Read));
    assert_eq!(t.schema["$id"], SCHEMA_ID);
    assert_eq!(t.drop_in_schema.as_ref().unwrap()["$id"], DROP_IN_SCHEMA_ID);
    let mut maps = t.key_maps.clone();
    maps.sort();
    assert_eq!(
        maps,
        ["/clients", "/services", "/visitors", "/web/forward_auth"]
    );
}
