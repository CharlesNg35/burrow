#![cfg(test)]

use burrow_admin_proto::msg::{AdminLevel, ErrorCode, HistoryRequest};
use burrow_admin_proto::ops::Op;
use serde_json::json;

use crate::admin::core::plan;
use crate::admin::core::*;

use super::{World, apply_now, core_with_env, p, session, world};

const VARS: &[(&str, &str)] = &[("OLD", "old-value"), ("HOME", "home-secret")];

fn refs_world() -> (World, String) {
    let mut w = world();
    w.text = w.text.replace(
        "\nservices:\n",
        "\nweb:\n  http: 8080\n  letsencrypt:\n    email: ops@example.com\n\nservices:\n  site:\n    client: home\n    target: 9000\n    web:\n      hosts: [site.example.com]\n      response_headers:\n        X-Old: \"${OLD}\"\n",
    );
    let running = w.text.replace("${OLD}", "old-value");
    (w, running)
}

fn header(name: &str) -> Vec<String> {
    p(&["services", "site", "web", "response_headers", name])
}

fn refused(core: &Core, s: &Session, ops: &[Op]) -> String {
    let base = text::rev(&core.read().unwrap());
    match plan::compute(core, s, &base, ops) {
        Err(e) if e.code == ErrorCode::Forbidden => e.message,
        Err(e) => panic!("refused for another reason: {}", e.message),
        Ok(c) => panic!("not refused: {}", c.new_text),
    }
}

#[test]
fn an_admin_plan_never_adds_an_environment_or_file_reference() {
    let (w, running) = refs_world();
    let core = core_with_env(&w.text, &running, VARS);
    let s = session(&w, AdminLevel::Write);
    let site = |k: &str| p(&["services", "site", "web", k]);
    let cases = [
        (
            Op::Set {
                path: header("X-Leak"),
                value: json!("${HOME}"),
            },
            "${HOME}",
        ),
        (
            Op::Set {
                path: site("redirects"),
                value: json!({"/go": "https://evil.example/${HOME}"}),
            },
            "${HOME}",
        ),
        (
            Op::Set {
                path: site("request_headers"),
                value: json!({"X-Token": "${HOME}"}),
            },
            "${HOME}",
        ),
        (
            Op::Set {
                path: p(&["web", "letsencrypt", "email"]),
                value: json!("${HOME}@evil.example"),
            },
            "${HOME}",
        ),
        (
            Op::Set {
                path: header("X-File"),
                value: json!("file:/etc/shadow"),
            },
            "file:/etc/shadow",
        ),
        (
            Op::Set {
                path: site("response_headers"),
                value: json!({"X-New": "${OLD}"}),
            },
            "${OLD}",
        ),
    ];
    for (op, what) in cases {
        let message = refused(&core, &s, std::slice::from_ref(&op));
        assert!(message.contains(what), "{what}: {message}");
        assert!(message.contains("server.yaml"), "{message}");
    }
}

#[test]
fn an_admin_plan_keeps_or_removes_the_references_already_written() {
    let (w, running) = refs_world();
    let core = core_with_env(&w.text, &running, VARS);
    let s = session(&w, AdminLevel::Write);
    let base = || text::rev(&core.read().unwrap());
    for op in [
        Op::Set {
            path: p(&["services", "site", "target"]),
            value: json!(9001),
        },
        Op::Set {
            path: header("X-Old"),
            value: json!("prefix ${OLD}"),
        },
        Op::Remove {
            path: header("X-Old"),
        },
        Op::Rename {
            path: p(&["services", "site"]),
            to: "blog".into(),
            refs: true,
        },
    ] {
        let c = plan::compute(&core, &s, &base(), std::slice::from_ref(&op));
        assert!(c.is_ok(), "{op:?}: {:?}", c.err().map(|e| e.message));
    }
}

#[test]
fn a_revert_brings_a_reference_back_only_where_the_file_still_has_it() {
    let (w, running) = refs_world();
    let core = core_with_env(&w.text, &running, VARS);
    let s = session(&w, AdminLevel::Write);
    apply_now(
        &core,
        &s,
        &[Op::Remove {
            path: header("X-Old"),
        }],
        &[],
    )
    .unwrap();
    let entry = core
        .history(&HistoryRequest {
            limit: None,
            diff: None,
            entry: None,
        })
        .unwrap()
        .entries
        .remove(0);
    let revert = [Op::ReplaceDocument {
        from_history: entry.base.clone(),
    }];
    let message = refused(&core, &s, &revert);
    assert!(message.contains("${OLD}"), "{message}");
    let again = core.read().unwrap().replace(
        "    web:\n",
        "    web:\n      request_headers:\n        X-Old: \"${OLD}\"\n",
    );
    core.store.write(&core.read().unwrap(), &again).unwrap();
    let still = core.read().unwrap();
    let c = plan::compute(&core, &s, &text::rev(&still), &revert);
    assert!(c.is_err(), "the reference sits at another path in the file");
    let back = w.text.replace(
        "      response_headers:\n",
        "      request_headers:\n        X-Other: plain\n      response_headers:\n",
    );
    core.store.write(&still, &back).unwrap();
    let c = plan::compute(&core, &s, &text::rev(&back), &revert);
    assert!(c.is_ok(), "{:?}", c.err().map(|e| e.message));
}
