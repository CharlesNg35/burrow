use burrow_admin_proto::codes::OLDER_ADMIN;
use burrow_admin_proto::local::UI_CLIENT;
use burrow_admin_proto::msg::{AdminLevel, WelcomeRequest, req};
use serde_json::json;

use super::{World, core_for, world};
use crate::admin::core::{Session, dispatch};

fn opened_by(w: &World, client: &str) -> Session {
    Session::new(
        "ops".into(),
        AdminLevel::Write,
        "203.0.113.7:5000".parse().unwrap(),
        w.ops.clone(),
        false,
        client.into(),
    )
}

fn with_entries(w: &World) -> String {
    w.text.replace(
        "web: { client: home, bind: 7840, target: 3000 }",
        "web: { clients: [home, { name: home, target: 3001, weight: 2 }], mode: balance, bind: 7840, target: 3000 }",
    )
}

fn notices(core: &super::super::Core, s: &Session) -> Vec<String> {
    core.state(s)
        .unwrap()
        .diags
        .into_iter()
        .filter(|d| d.code == OLDER_ADMIN)
        .map(|d| d.message)
        .collect()
}

#[test]
fn a_v0_1_2_admin_is_told_which_settings_it_cannot_edit_and_a_new_one_is_not() {
    let w = world();
    let core = core_for(&with_entries(&w));
    let old = opened_by(&w, &format!("{UI_CLIENT}/0.1.2"));
    dispatch(&core, &old, req::WELCOME, &json!({})).unwrap();
    let got = notices(&core, &old);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].starts_with("This burrow admin is older than the server (v")
            && got[0].ends_with("cannot edit some settings here: Clients of service web"),
        "{got:?}"
    );
    let new = opened_by(&w, &format!("{UI_CLIENT}/0.2.0"));
    let hello = serde_json::to_value(WelcomeRequest {
        burrow: Some("0.2.0".into()),
    })
    .unwrap();
    dispatch(&core, &new, req::WELCOME, &hello).unwrap();
    assert!(notices(&core, &new).is_empty());
}

#[test]
fn an_older_admin_on_a_config_it_can_edit_and_other_clients_get_no_notice() {
    let w = world();
    let old = opened_by(&w, &format!("{UI_CLIENT}/0.1.2"));
    dispatch(&core_for(&w.text), &old, req::WELCOME, &json!(null)).unwrap();
    assert!(notices(&core_for(&w.text), &old).is_empty());
    let entries = core_for(&with_entries(&w));
    let mcp = opened_by(&w, "burrow-mcp/0.1.2");
    dispatch(&entries, &mcp, req::WELCOME, &json!({})).unwrap();
    assert!(notices(&entries, &mcp).is_empty());
    let odd = opened_by(&w, &format!("{UI_CLIENT}/0.1.2"));
    dispatch(&entries, &odd, req::WELCOME, &json!({"burrow": 7})).unwrap();
    assert_eq!(
        notices(&entries, &odd).len(),
        1,
        "an unreadable version is no version"
    );
}

#[test]
fn the_welcome_request_carries_the_version_only_when_set() {
    let none = serde_json::to_value(WelcomeRequest::default()).unwrap();
    assert_eq!(none, json!({}));
    let some: WelcomeRequest =
        serde_json::from_value(json!({"burrow": "0.2.0", "later": 1})).unwrap();
    assert_eq!(some.burrow.as_deref(), Some("0.2.0"));
}
