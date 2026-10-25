use burrow_admin_proto::msg::{AdminLevel, ErrorCode, HashedPassword, req};
use burrow_admin_proto::ops::Op;
use burrow_config::{ServiceEntry, WebSite};
use serde_json::json;

use super::{World, core_for, session, world};
use crate::admin::core::{dispatch, plan, text};

fn with_site(w: &World, auth: &str) -> String {
    format!(
        "{}  site:\n    client: home\n    target: 3000\n    web:\n      hosts: [site.example.com]\n      tls: \"off\"\n{auth}web:\n  http: 7899\n",
        w.text
    )
}

fn auth_path(user: &str) -> Vec<String> {
    let mut path = ServiceEntry::WEB.of("site");
    path.push(WebSite::BASIC_AUTH.key().into());
    path.push(user.into());
    path
}

fn hashed(core: &crate::admin::core::Core, w: &World, password: &str) -> HashedPassword {
    let s = session(w, AdminLevel::Write);
    serde_json::from_value(
        dispatch(
            core,
            &s,
            req::HASH_PASSWORD,
            &serde_json::to_value(burrow_admin_proto::msg::HashPassword {
                password: password.into(),
            })
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn a_password_becomes_an_argon2id_hash_in_the_plan_and_never_appears_itself() {
    let w = world();
    let text = with_site(&w, "");
    let core = core_for(&text);
    let s = session(&w, AdminLevel::Write);
    let h: HashedPassword = serde_json::from_value(
        dispatch(
            &core,
            &s,
            req::HASH_PASSWORD,
            &serde_json::to_value(burrow_admin_proto::msg::HashPassword {
                password: "correct horse".into(),
            })
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let ops = [Op::PasswordSet {
        path: auth_path("alice"),
        handle: h.handle.clone(),
    }];
    let c = plan::compute(&core, &s, &text::rev(&text), &ops).unwrap();
    assert!(c.plan.valid, "{:?}", c.plan.diags);
    assert!(
        c.new_text
            .contains("alice: \"$argon2id$v=19$m=19456,t=2,p=1$"),
        "{}",
        c.new_text
    );
    assert!(!c.new_text.contains("correct horse"));
    assert!(
        !serde_json::to_string(&c.plan)
            .unwrap()
            .contains("correct horse")
    );
    let shown = serde_json::to_string(&c.plan).unwrap();
    assert!(!shown.contains("$argon2id$"), "{shown}");
    assert!(shown.contains("«password hash»"), "{shown}");
    assert_eq!(c.passwords, [h.handle]);
}

#[test]
fn a_stored_hash_shows_only_as_set() {
    let w = world();
    let text = with_site(
        &w,
        "      basic_auth:\n        alice: \"$2b$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy\"\n",
    );
    let core = core_for(&text);
    let st = core.state(&session(&w, AdminLevel::Read)).unwrap();
    let mut at = st.values.clone();
    for k in auth_path("alice") {
        at = at[&k].clone();
    }
    assert_eq!(at, json!({ "$hash": {} }));
    assert!(
        !serde_json::to_string(&st.values)
            .unwrap()
            .contains("N9qo8u")
    );
}

#[test]
fn passwords_are_for_write_admins_and_their_handles_are_not_secrets() {
    let w = world();
    let text = with_site(&w, "");
    let core = core_for(&text);
    let read = session(&w, AdminLevel::Read);
    let e = dispatch(
        &core,
        &read,
        req::HASH_PASSWORD,
        &serde_json::to_value(burrow_admin_proto::msg::HashPassword {
            password: "x".into(),
        })
        .unwrap(),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden);
    let s = session(&w, AdminLevel::Write);
    let e = dispatch(
        &core,
        &s,
        req::HASH_PASSWORD,
        &serde_json::to_value(burrow_admin_proto::msg::HashPassword {
            password: String::new(),
        })
        .unwrap(),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    let secret = core.generate_secret(&s).unwrap().handle;
    let as_password = [Op::PasswordSet {
        path: auth_path("alice"),
        handle: secret,
    }];
    assert!(plan::compute(&core, &s, &text::rev(&text), &as_password).is_err());
    let h = hashed(&core, &w, "x");
    let as_secret = [Op::SecretAdd {
        path: burrow_config::ClientEntry::SECRET.of("home"),
        handle: h.handle,
    }];
    assert!(plan::compute(&core, &s, &text::rev(&text), &as_secret).is_err());
    let marker = [Op::Set {
        path: auth_path("alice"),
        value: json!({ "$hash": {} }),
    }];
    assert!(plan::compute(&core, &s, &text::rev(&text), &marker).is_err());
}
