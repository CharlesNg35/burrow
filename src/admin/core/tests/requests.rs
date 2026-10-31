use burrow_admin_proto::msg::{AdminLevel, ErrorCode, Suggest, req, suggest};
use burrow_config::ClientEntry;
use burrow_config::field::Entry as _;
use serde_json::{Value, json};

use super::{core_busy, core_for, session, world};
use crate::admin::core::dispatch;

#[test]
fn every_request_but_apply_and_subscribe_reaches_a_handler_in_dispatch() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    for t in req::ALL
        .iter()
        .filter(|t| ![req::APPLY, req::SUBSCRIBE].contains(t))
    {
        if let Err(e) = dispatch(&core, &s, t, &json!({})) {
            assert_ne!(e.code, ErrorCode::UnknownRequest, "{t}: {}", e.message);
        }
    }
}

fn ask(what: &str, near: Option<u32>, base: Option<&str>) -> Suggest {
    Suggest {
        what: what.into(),
        near,
        count: None,
        proto: None,
        base: base.map(str::to_string),
        entity: None,
    }
}

#[test]
fn a_free_port_suggestion_skips_ports_the_config_already_binds() {
    let w = world();
    let core = core_for(&w.text);
    let got = core.suggest(&ask(suggest::FREE_PORT, Some(7840), None));
    assert_eq!(got.unwrap().values, vec![Value::from(7841)]);
}

#[test]
fn no_free_port_on_the_machine_gives_an_empty_suggestion() {
    let w = world();
    let core = core_busy(&w.text, &w.text, vec![7900]);
    let got = core.suggest(&ask(suggest::FREE_PORT, Some(7900), None));
    assert!(got.unwrap().values.is_empty());
}

#[test]
fn a_name_suggestion_numbers_a_name_already_taken() {
    let w = world();
    let core = core_for(&w.text);
    let client = Suggest {
        entity: Some(ClientEntry::ENTITY.into()),
        ..ask(suggest::NAME, None, Some("home"))
    };
    let got = core.suggest(&client);
    assert_eq!(got.unwrap().values, vec![Value::from("home-2")]);
    let free = core.suggest(&ask(suggest::NAME, None, Some("blog")));
    assert_eq!(free.unwrap().values, vec![Value::from("blog")]);
}

#[test]
fn an_unknown_suggestion_is_a_bad_request() {
    let w = world();
    let core = core_for(&w.text);
    let e = core.suggest(&ask("colour", None, None)).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
}
