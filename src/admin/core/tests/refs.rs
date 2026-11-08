#![cfg(test)]

use burrow_admin_proto::msg::AdminLevel;
#[cfg(feature = "web")]
use burrow_admin_proto::msg::HistoryRequest;
use burrow_admin_proto::ops::Op;
use serde_json::json;

use crate::admin::core::plan;
use crate::admin::core::*;

#[cfg(feature = "web")]
use super::apply_now;
use super::{World, core_with_env, p, session, world};

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

#[test]
fn an_admin_plan_writes_environment_and_file_references_like_the_file() {
    let (w, running) = refs_world();
    let core = core_with_env(&w.text, &running, VARS);
    let s = session(&w, AdminLevel::Write);
    let base = || text::rev(&core.read().unwrap());
    let site = |k: &str| p(&["services", "site", "web", k]);
    for (op, written) in [
        (
            Op::Set {
                path: header("X-New"),
                value: json!("${HOME}"),
            },
            "${HOME}",
        ),
        (
            Op::Set {
                path: site("request_headers"),
                value: json!({"X-Token": "${OLD}"}),
            },
            "${OLD}",
        ),
        (
            Op::Set {
                path: p(&["web", "letsencrypt", "email"]),
                value: json!("${HOME}@example.com"),
            },
            "${HOME}@example.com",
        ),
        (
            Op::Set {
                path: header("X-File"),
                value: json!("file:header.txt"),
            },
            "file:header.txt",
        ),
    ] {
        let c = plan::compute(&core, &s, &base(), std::slice::from_ref(&op))
            .unwrap_or_else(|e| panic!("{op:?}: {}", e.message));
        assert!(
            c.new_docs.main().text.contains(written),
            "{}",
            c.new_docs.main().text
        );
        let shown = format!(
            "{}{}",
            c.plan.diff.unified,
            serde_json::to_string(&c.plan.changes).unwrap()
        );
        assert!(
            !shown.contains("home-secret") && !shown.contains("old-value"),
            "a reference shows as written, never its value: {shown}"
        );
    }
}

#[cfg(feature = "web")]
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

#[cfg(feature = "web")]
#[test]
fn a_revert_brings_a_removed_reference_back() {
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
    let now = core.read().unwrap();
    let c = plan::compute(&core, &s, &text::rev(&now), &revert)
        .unwrap_or_else(|e| panic!("{}", e.message));
    let main = &c.new_docs.main().text;
    assert!(main.contains("X-Old: \"${OLD}\""), "{main}");
}
