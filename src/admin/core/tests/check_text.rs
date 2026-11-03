use burrow_admin_proto::check::MAX_TEXT;
use burrow_admin_proto::msg::{AdminLevel, ErrorCode, req};
use serde_json::{Value, json};

use super::{core_for, core_with_env, session, world};
use crate::admin::core::dispatch;

fn messages(v: &Value) -> Vec<String> {
    v["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("{v}"))
        .iter()
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_read_admin_checks_text_with_the_server_s_env_and_nothing_is_written() {
    let w = world();
    let core = core_with_env(
        &w.text,
        &w.text,
        &[("LISTEN_AT", "0.0.0.0:not-a-port-hidden")],
    );
    let s = session(&w, AdminLevel::Read);
    let text = "listen: ${LISTEN_AT}\nmetrics: ${UNSET_ON_THIS_SERVER}\n";
    let v = dispatch(&core, &s, req::CHECK_TEXT, &json!({ "text": text })).unwrap();
    assert_eq!(v["valid"], false, "{v}");
    let said = messages(&v);
    assert!(
        said.iter()
            .any(|m| m == "environment variable \"UNSET_ON_THIS_SERVER\" is not set"),
        "the server checks its own env: {v}"
    );
    assert!(!v.to_string().contains("not-a-port-hidden"), "{v}");
    let bare = "listen: 127.0.0.1:7835\n";
    let part = dispatch(&core, &s, req::CHECK_TEXT, &json!({ "text": bare })).unwrap();
    assert!(
        !messages(&part).iter().any(|m| m.contains("no clients")),
        "a fragment: {part}"
    );
    let whole = dispatch(
        &core,
        &s,
        req::CHECK_TEXT,
        &json!({ "text": bare, "scope": "whole" }),
    )
    .unwrap();
    assert!(
        messages(&whole).iter().any(|m| m.contains("no clients")),
        "{whole}"
    );
    assert_eq!(core.store.read().unwrap(), w.text, "nothing is written");
    let e = dispatch(&core, &s, req::VALIDATE, &json!({ "base": "x", "ops": [] })).unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden, "VALIDATE stays write-only");
}

#[test]
fn text_over_the_size_cap_is_refused_before_it_is_read() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Read);
    let big = "#".repeat(MAX_TEXT + 1);
    let e = dispatch(&core, &s, req::CHECK_TEXT, &json!({ "text": big })).unwrap_err();
    assert_eq!(e.code, ErrorCode::TooLarge);
    let ok = dispatch(
        &core,
        &s,
        req::CHECK_TEXT,
        &json!({ "text": w.text, "scope": "whole" }),
    );
    assert_eq!(ok.unwrap()["valid"], true);
}
