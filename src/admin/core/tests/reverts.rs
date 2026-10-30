#![cfg(test)]

use burrow_admin_proto::msg::{AdminLevel, Apply, ErrorCode, HistoryRequest};
use burrow_admin_proto::ops::Op;
use serde_json::json;

use crate::admin::core::plan::{self, ReloadResult};
use crate::admin::core::*;

use super::{apply_now, core_counted, core_for, core_running, p, session, world};

#[test]
fn an_applied_revert_is_recorded_with_the_rev_it_went_back_to() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    apply_now(
        &core,
        &s,
        &[Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3001),
        }],
        &[],
    )
    .unwrap();
    let all = || {
        core.history(&HistoryRequest {
            limit: None,
            diff: None,
            entry: None,
        })
        .unwrap()
        .entries
    };
    let changed = all().remove(0);
    assert_eq!(changed.reverts, None);
    apply_now(
        &core,
        &s,
        &[Op::ReplaceDocument {
            from_history: changed.base.clone(),
        }],
        &[],
    )
    .unwrap();
    let back = all().remove(0);
    assert_eq!(back.reverts.as_deref(), Some(changed.base.as_str()));
    assert_eq!(back.rev, changed.base);
}

#[test]
fn a_revert_is_an_ordinary_plan() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    apply_now(
        &core,
        &s,
        &[Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3001),
        }],
        &[],
    )
    .unwrap();
    let now = core.read().unwrap();
    let entry = core
        .history(&HistoryRequest {
            limit: None,
            diff: None,
            entry: None,
        })
        .unwrap()
        .entries
        .remove(0);
    let reply = dispatch(
        &core,
        &s,
        "revert",
        &json!({"to": entry.base, "base": text::rev(&now)}),
    )
    .unwrap();
    let plan: burrow_admin_proto::plan::Plan = serde_json::from_value(reply).unwrap();
    assert!(plan.valid);
    assert_eq!(plan.result_rev, text::rev(&w.text));
    let with_diff = core
        .history(&HistoryRequest {
            limit: None,
            diff: None,
            entry: Some(entry.id),
        })
        .unwrap();
    assert!(with_diff.diff.unwrap().unified.contains("target: 3001"));
    apply_now(
        &core,
        &s,
        &[Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3002),
        }],
        &[],
    )
    .unwrap();
    let older = core
        .history(&HistoryRequest {
            limit: Some(1),
            diff: None,
            entry: Some(entry.id),
        })
        .expect("an older entry's diff is found whatever the limit");
    assert!(older.diff.unwrap().unified.contains("target: 3001"));
}

#[test]
fn locked_fields_are_refused_even_from_a_crafted_client() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&w.text);
    for (path, value) in [
        (p(&["listen"]), json!(7836)),
        (p(&["insecure_plaintext"]), json!(true)),
        (p(&["heartbeat", "interval_secs"]), json!(20)),
        (p(&["limits", "handshake_timeout_secs"]), json!(9)),
        (p(&["limits", "max_handshakes"]), json!(9)),
        (p(&["heartbeat"]), json!({"interval_secs": 20})),
    ] {
        let e = plan::compute(
            &core,
            &s,
            &base,
            &[Op::Set {
                path: path.clone(),
                value,
            }],
        )
        .err()
        .unwrap_or_else(|| panic!("{path:?} was not refused"));
        assert_eq!(e.code, ErrorCode::LockedField, "{path:?}");
        assert!(
            e.message.contains("server.yaml on the server"),
            "{}",
            e.message
        );
    }
    let e = plan::compute(
        &core,
        &s,
        &base,
        &[Op::Remove {
            path: p(&["listen"]),
        }],
    )
    .err()
    .unwrap();
    assert_eq!(e.code, ErrorCode::LockedField);
    let e = plan::compute(
        &core,
        &s,
        &base,
        &[Op::SetText {
            path: p(&["websocket"]),
            text: "listen: 1\ntls: { cert: a.pem, key: a.key }\n".into(),
        }],
    )
    .err()
    .unwrap();
    assert_eq!(e.code, ErrorCode::LockedField);
    assert_eq!(core.read().unwrap(), w.text, "nothing was written");
    assert_eq!(
        core.schema.locked_paths(),
        vec![
            p(&["listen"]),
            p(&["websocket", "tls", "cert"]),
            p(&["websocket", "tls", "key"]),
            p(&["web", "letsencrypt", "ca"]),
            p(&["web", "letsencrypt", "dns", "*", "cloudflare", "token"]),
            p(&["web", "letsencrypt", "dns", "*", "rfc2136", "server"]),
            p(&["web", "letsencrypt", "dns", "*", "rfc2136", "key_name"]),
            p(&["web", "letsencrypt", "dns", "*", "rfc2136", "secret"]),
            p(&["web", "letsencrypt", "dns", "*", "rfc2136", "algorithm"]),
            p(&["web", "letsencrypt", "dns", "*", "acme_dns", "api"]),
            p(&["web", "letsencrypt", "dns", "*", "acme_dns", "username"]),
            p(&["web", "letsencrypt", "dns", "*", "acme_dns", "password"]),
            p(&["web", "letsencrypt", "dns", "*", "command"]),
            p(&["web", "certificates", "*", "cert"]),
            p(&["web", "certificates", "*", "key"]),
            p(&["web", "proxy_protocol_from"]),
            p(&["web", "access_log"]),
            p(&["web", "error_pages"]),
            p(&["services", "*", "web", "tls", "cert"]),
            p(&["services", "*", "web", "tls", "key"]),
            p(&["services", "*", "web", "target_tls", "ca"]),
            p(&["services", "*", "web", "client_auth", "ca"]),
            p(&["services", "*", "web", "client_auth", "crl"]),
            p(&["services", "*", "web", "error_pages"]),
            p(&["insecure_plaintext"]),
            p(&["heartbeat", "interval_secs"]),
            p(&["limits", "handshake_timeout_secs"]),
            p(&["limits", "max_handshakes"]),
        ]
    );
}

#[test]
fn a_revert_keeps_the_current_values_of_locked_fields() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    apply_now(
        &core,
        &s,
        &[Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3001),
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
    let edited = core
        .read()
        .unwrap()
        .replace("listen: 7835 # tcp", "listen: 7999 # tcp");
    core.store.write(&core.read().unwrap(), &edited).unwrap();
    let reply = dispatch(
        &core,
        &s,
        "revert",
        &json!({"to": entry.base, "base": text::rev(&edited)}),
    )
    .unwrap();
    let plan: burrow_admin_proto::plan::Plan = serde_json::from_value(reply).unwrap();
    assert!(plan.valid, "{:?}", plan.diags);
    let kept = plan
        .impacts
        .iter()
        .find(|i| i.code == burrow_admin_proto::codes::LOCKED_KEPT)
        .expect("a note");
    assert_eq!(kept.message, "listen kept at 7999 (locked)");
    let computed = plan::compute(
        &core,
        &s,
        &text::rev(&edited),
        &[Op::ReplaceDocument {
            from_history: entry.base.clone(),
        }],
    )
    .unwrap();
    assert!(
        computed.new_text.contains("listen: 7999 # tcp"),
        "{}",
        computed.new_text
    );
    assert!(
        computed.new_text.contains("target: 3000"),
        "{}",
        computed.new_text
    );
}

#[test]
fn a_reload_that_would_clash_with_the_kept_listener_is_predicted() {
    let w = world();
    let edited = w.text.replace("listen: 7835 # tcp", "listen: 7836 # tcp");
    let core = core_running(&edited, &w.text);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&edited);
    let ops = [Op::Set {
        path: p(&["services", "web", "bind"]),
        value: json!(7835),
    }];
    let c = plan::compute(&core, &s, &base, &ops).unwrap();
    assert!(!c.plan.valid, "{:?}", c.plan.diags);
    let d = c
        .plan
        .diags
        .iter()
        .find(|d| d.code == burrow_admin_proto::codes::LISTEN_CLASH)
        .unwrap();
    assert_eq!(d.path.as_deref().unwrap(), p(&["services", "web", "bind"]));
}

#[test]
fn set_text_is_parsed_by_the_server() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&w.text);
    let c = plan::compute(
        &core,
        &s,
        &base,
        &[Op::SetText {
            path: p(&["pool"]),
            text: "min_idle: 2\n".into(),
        }],
    )
    .unwrap();
    assert!(c.plan.valid, "{:?}", c.plan.diags);
    assert!(c.new_text.contains("pool:\n  min_idle: 2\n"));
    let locked = plan::compute(
        &core,
        &s,
        &base,
        &[Op::SetText {
            path: p(&["limits"]),
            text: "max_pending: 9\nmax_handshakes: 9\n".into(),
        }],
    )
    .err()
    .unwrap();
    assert_eq!(
        locked.code,
        ErrorCode::LockedField,
        "a block that changes a locked field inside is locked"
    );
    let kept = plan::compute(
        &core,
        &s,
        &base,
        &[Op::SetText {
            path: p(&["limits"]),
            text: "max_pending: 9\n".into(),
        }],
    );
    assert!(
        kept.is_ok(),
        "a block that leaves its locked fields as they are is not"
    );
    assert!(
        plan::compute(
            &core,
            &s,
            &base,
            &[Op::SetText {
                path: p(&["limits"]),
                text: "[a".into()
            }]
        )
        .is_err()
    );
}

#[test]
fn a_rejected_reload_is_reported_with_the_restore() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&w.text);
    let ops = [Op::Set {
        path: p(&["services", "web", "target"]),
        value: json!(3001),
    }];
    let plan = plan::compute(&core, &s, &base, &ops).unwrap().plan;
    let begun = plan::apply_begin(
        &core,
        &s,
        &Apply {
            base: base.clone(),
            ops: ops.to_vec(),
            plan: plan.plan,
            confirm: Vec::new(),
            address: None,
        },
    )
    .unwrap();
    assert!(begun.wrote);
    assert!(plan::restore(&core, &begun));
    assert_eq!(core.read().unwrap(), w.text, "the previous text is back");
    let e = plan::finish(
        &core,
        &s,
        begun,
        ReloadResult::Rejected("clash".into()),
        Some(true),
    )
    .err()
    .unwrap();
    assert_eq!(e.code, ErrorCode::ReloadFailed);
    assert_eq!(e.body["restored"], true);
    let h = core
        .history(&HistoryRequest {
            limit: None,
            diff: None,
            entry: None,
        })
        .unwrap();
    assert_eq!(h.entries[0].result, "rejected, restored");
}

#[test]
fn the_same_file_twice_gives_two_entries_each_with_its_own_diff() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    for target in [3001, 3002, 3001] {
        apply_now(
            &core,
            &s,
            &[Op::Set {
                path: p(&["services", "web", "target"]),
                value: json!(target),
            }],
            &[],
        )
        .unwrap();
    }
    let all = core
        .history(&HistoryRequest {
            limit: None,
            diff: None,
            entry: None,
        })
        .unwrap()
        .entries;
    let (newer, older) = (&all[0], &all[2]);
    assert_eq!(
        newer.rev, older.rev,
        "A -> B -> A writes the same file twice"
    );
    assert_ne!(newer.id, older.id);
    assert!(
        all.windows(2).all(|p| p[0].id > p[1].id),
        "ids grow with time"
    );
    let diff_of = |id: u64| {
        core.history(&HistoryRequest {
            limit: Some(1),
            diff: None,
            entry: Some(id),
        })
        .unwrap()
        .diff
        .unwrap()
        .unified
    };
    let (n, o) = (diff_of(newer.id), diff_of(older.id));
    assert!(
        n.contains("-") && n.contains("3002"),
        "the newer one starts from 3002: {n}"
    );
    assert!(
        !o.contains("3002"),
        "the older one starts from the first file: {o}"
    );
    assert!(o.contains("3001"), "{o}");
    let by_rev = core
        .history(&HistoryRequest {
            limit: Some(1),
            diff: Some(newer.rev.clone()),
            entry: None,
        })
        .unwrap()
        .diff
        .unwrap()
        .unified;
    assert_eq!(
        by_rev, n,
        "a v0.1.2 UI asks by rev and gets the newest entry with it"
    );
    let old_shape: HistoryRequest = serde_json::from_value(json!({"diff": older.rev})).unwrap();
    assert_eq!(old_shape.diff.as_deref(), Some(older.rev.as_str()));
    let missing = core.history(&HistoryRequest {
        limit: None,
        diff: Some("b2s:none".into()),
        entry: None,
    });
    assert!(missing.is_err());
    let now = core.read().unwrap();
    for e in [newer, older] {
        let reply = dispatch(
            &core,
            &s,
            "revert",
            &json!({"to": e.base, "base": text::rev(&now)}),
        )
        .unwrap();
        let plan: burrow_admin_proto::plan::Plan = serde_json::from_value(reply).unwrap();
        assert!(plan.valid, "a revert works from either entry");
        assert_eq!(plan.result_rev, e.base);
    }
}

#[test]
fn history_asks_the_store_once_per_rev() {
    let w = world();
    let (core, asked) = core_counted(&w.text, &w.text, Vec::new(), Vec::new());
    let revs = ["b2s:a", "b2s:b", "b2s:c"];
    core.store.keep(revs[0], "listen: 1\n");
    core.store.keep(revs[1], "listen: 2\n");
    for i in 0..30u64 {
        core.record(burrow_admin_proto::msg::AuditEntry {
            at: i,
            rev: revs[i as usize % 3].into(),
            result: "applied".into(),
            ..Default::default()
        });
    }
    let entries = core
        .history(&HistoryRequest {
            limit: Some(30),
            ..Default::default()
        })
        .unwrap()
        .entries;
    assert_eq!(entries.len(), 30);
    for e in &entries {
        assert_eq!(e.revertible, e.rev != revs[2], "{}", e.rev);
    }
    assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 3);
}
