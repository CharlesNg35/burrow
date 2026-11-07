#![cfg(test)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use burrow_admin_proto::msg::{AdminLevel, AuditEntry, HistoryRequest};
use burrow_admin_proto::ops::Op;
use serde_json::json;

use crate::admin::adapters::FileStore;
use crate::admin::core::*;
use crate::metrics::Status;

use super::{FakeLive, apply_now, p, session, world};

fn started(path: &Path) -> Core {
    let text = std::fs::read_to_string(path).unwrap();
    Core::new(
        Box::new(FileStore::new(path)),
        Box::new(FakeLive {
            text: Arc::new(Mutex::new(text)),
            status: Mutex::new(Status::default()),
            sessions: Mutex::new(Vec::new()),
            busy: Vec::new(),
            allows: BTreeMap::new(),
        }),
        Box::new(|_: &str| None),
    )
}

fn activity(core: &Core) -> Vec<AuditEntry> {
    core.history(&HistoryRequest {
        limit: None,
        diff: None,
        entry: None,
    })
    .unwrap()
    .entries
}

fn target(port: u64) -> Vec<Op> {
    vec![Op::Set {
        path: p(&["services", "web", "target"]),
        value: json!(port),
    }]
}

fn by_hand(e: &AuditEntry) -> bool {
    e.visitor.is_empty() && e.from.is_empty() && e.changes.is_empty() && e.result == "kept"
}

#[test]
fn an_admin_change_keeps_who_where_and_what_after_a_restart() {
    let w = world();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    std::fs::write(&path, &w.text).unwrap();
    let s = session(&w, AdminLevel::Write);
    let before = {
        let core = started(&path);
        apply_now(&core, &s, &target(3001), &[]).unwrap();
        activity(&core)
    };
    let after = activity(&started(&path));
    assert_eq!(after.len(), 2, "{after:#?}");
    let (was, now) = (&before[0], &after[0]);
    assert_eq!(now.visitor, "ops");
    assert_eq!(now.from, "203.0.113.7");
    assert!(!now.changes.is_empty());
    assert_eq!(now.changes, was.changes);
    assert_eq!(
        (&now.base, &now.rev, &now.result, now.at),
        (&was.base, &was.rev, &was.result, was.at)
    );
    assert!(now.details_kept && now.revertible);
    assert!(by_hand(&after[1]), "{:#?}", after[1]);
    let history = crate::config::save::history_dir_of(&path);
    for f in std::fs::read_dir(&history).unwrap().flatten() {
        let body = std::fs::read_to_string(f.path()).unwrap();
        if f.file_name().to_string_lossy().starts_with("audit-") {
            assert!(!body.contains(&w.ops.encode()) && !body.contains(&w.home.encode()));
        }
    }
}

#[test]
fn a_revert_and_a_hand_edit_read_as_before_after_a_restart() {
    let w = world();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    std::fs::write(&path, &w.text).unwrap();
    let s = session(&w, AdminLevel::Write);
    let first = {
        let core = started(&path);
        apply_now(&core, &s, &target(3001), &[]).unwrap();
        activity(&core).remove(0)
    };
    {
        let core = started(&path);
        apply_now(
            &core,
            &s,
            &[Op::ReplaceDocument {
                from_history: first.base.clone(),
            }],
            &[],
        )
        .unwrap();
    }
    let back = activity(&started(&path));
    assert_eq!(back[0].reverts.as_deref(), Some(first.base.as_str()));
    assert_eq!(
        (back[0].visitor.as_str(), back[0].rev.as_str()),
        ("ops", first.base.as_str())
    );
    let edited = w.text.replace("target: 3000", "target: 3005");
    assert_ne!(edited, w.text);
    std::fs::write(&path, &edited).unwrap();
    {
        let core = started(&path);
        apply_now(&core, &s, &target(3006), &[]).unwrap();
    }
    let all = activity(&started(&path));
    assert_eq!(all[0].visitor, "ops");
    assert_eq!(all[0].base, all[1].rev);
    assert!(by_hand(&all[1]), "{:#?}", all[1]);
    assert_eq!(
        all.iter().filter(|e| e.visitor == "ops").count(),
        3,
        "{all:#?}"
    );
}
