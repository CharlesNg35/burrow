#![cfg(test)]

use burrow_admin_proto::msg::{AdminLevel, ErrorCode, HistoryRequest};
use burrow_admin_proto::ops::Op;
use serde_json::json;

use crate::admin::core::plan;
use crate::admin::core::*;

use super::{World, apply_now, core_for, p, session, world};

const BEFORE_P2: [&[&str]; 7] = [
    &["listen"],
    &["websocket"],
    &["web"],
    &["insecure_plaintext"],
    &["heartbeat", "interval_secs"],
    &["limits", "handshake_timeout_secs"],
    &["limits", "max_handshakes"],
];

struct Site {
    w: World,
    dir: tempfile::TempDir,
}

impl Site {
    fn file(&self, name: &str) -> String {
        self.dir.path().join(name).display().to_string()
    }
}

fn pair(dir: &std::path::Path, stem: &str) {
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["nas.example.com".to_string()]).unwrap();
    let cert = params.self_signed(&key).unwrap();
    std::fs::write(dir.join(format!("{stem}.pem")), cert.pem()).unwrap();
    std::fs::write(dir.join(format!("{stem}.key")), key.serialize_pem()).unwrap();
}

fn site() -> Site {
    let dir = tempfile::tempdir().unwrap();
    pair(dir.path(), "a");
    pair(dir.path(), "b");
    let mut w = world();
    let (cert, key) = (
        dir.path().join("a.pem").display().to_string(),
        dir.path().join("a.key").display().to_string(),
    );
    w.text = w.text.replace(
        "\nservices:\n",
        &format!(
            "\nweb:\n  https: 8443\n\nservices:\n  nas:\n    client: home\n    target: 443\n    web:\n      hosts: [nas.example.com]\n      tls: {{cert: {cert}, key: {key}}}\n"
        ),
    );
    Site { w, dir }
}

fn refused(core: &Core, s: &Session, ops: &[Op]) -> Vec<String> {
    let base = text::rev(&core.read().unwrap());
    match plan::compute(core, s, &base, ops) {
        Err(e) if e.code == ErrorCode::LockedField => {
            e.message.split(' ').take(1).map(str::to_string).collect()
        }
        Err(e) => panic!("refused for another reason: {}", e.message),
        Ok(c) => panic!("not refused: {}", c.new_text),
    }
}

#[test]
fn every_field_locked_before_stays_locked_and_files_in_entries_are_locked_too() {
    let core = core_for(&world().text);
    let now = core.schema.locked_paths();
    for old in BEFORE_P2 {
        assert!(now.contains(&p(old)), "{old:?} is still locked: {now:?}");
    }
    for f in ["cert", "key"] {
        let path = p(&["services", "*", "web", "tls", f]);
        assert!(now.contains(&path), "{path:?}: {now:?}");
    }
    assert_eq!(now.len(), BEFORE_P2.len() + 2, "{now:?}");
}

#[test]
fn a_sites_own_certificate_files_are_refused_on_every_admin_path() {
    let site = site();
    let core = core_for(&site.w.text);
    let s = session(&site.w, AdminLevel::Write);
    let other = site.file("b.pem");
    let tls = |cert: &str| json!({"cert": cert, "key": site.file("b.key")});
    let cases = [
        vec![Op::Set {
            path: p(&["services", "nas", "web", "tls", "cert"]),
            value: json!(other),
        }],
        vec![Op::Set {
            path: p(&["services", "nas", "web", "tls"]),
            value: tls(&other),
        }],
        vec![Op::Set {
            path: p(&["services", "nas", "web"]),
            value: json!({"hosts": ["nas.example.com"], "tls": tls(&other)}),
        }],
        vec![Op::Set {
            path: p(&["services", "new"]),
            value: json!({"client": "home", "target": 80, "web": {"hosts": ["new.example.com"], "tls": tls(&other)}}),
        }],
        vec![Op::SetText {
            path: p(&["services", "nas"]),
            text: format!(
                "client: home\ntarget: 443\nweb:\n  hosts: [nas.example.com]\n  tls: {{cert: {other}, key: {}}}\n",
                site.file("b.key")
            ),
        }],
    ];
    for ops in cases {
        let what = refused(&core, &s, &ops);
        assert!(what[0].contains(".web.tls."), "{what:?} for {ops:?}");
    }
}

#[test]
fn edits_that_keep_the_files_are_allowed() {
    let site = site();
    let core = core_for(&site.w.text);
    let s = session(&site.w, AdminLevel::Write);
    let base = || text::rev(&core.read().unwrap());
    let fine = [
        vec![Op::Set {
            path: p(&["services", "nas", "web", "hosts"]),
            value: json!(["nas.example.com", "files.example.com"]),
        }],
        vec![Op::Rename {
            path: p(&["services", "nas"]),
            to: "storage".into(),
            refs: true,
        }],
        vec![Op::Set {
            path: p(&["services", "nas", "web", "tls"]),
            value: json!("passthrough"),
        }],
        vec![Op::Remove {
            path: p(&["services", "nas"]),
        }],
    ];
    for ops in fine {
        let c = plan::compute(&core, &s, &base(), &ops);
        assert!(c.is_ok(), "{ops:?}: {:?}", c.err().map(|e| e.message));
    }
}

#[test]
fn the_files_stay_editable_by_file_and_a_revert_keeps_the_current_ones() {
    let site = site();
    let core = core_for(&site.w.text);
    let s = session(&site.w, AdminLevel::Write);
    apply_now(
        &core,
        &s,
        &[Op::Set {
            path: p(&["services", "nas", "target"]),
            value: json!(8443),
        }],
        &[],
    )
    .unwrap();
    let entry = core
        .history(&HistoryRequest {
            limit: None,
            diff: None,
        })
        .unwrap()
        .entries
        .remove(0);
    let edited = core
        .read()
        .unwrap()
        .replace(&site.file("a.pem"), &site.file("b.pem"));
    core.store.write(&core.read().unwrap(), &edited).unwrap();
    let edit = plan::compute(
        &core,
        &s,
        &text::rev(&edited),
        &[Op::Set {
            path: p(&["services", "nas", "target"]),
            value: json!(9443),
        }],
    );
    assert!(edit.is_ok(), "{:?}", edit.err().map(|e| e.message));
    let reverted = plan::compute(
        &core,
        &s,
        &text::rev(&edited),
        &[Op::ReplaceDocument {
            from_history: entry.base.clone(),
        }],
    )
    .unwrap();
    assert!(
        reverted.new_text.contains(&site.file("b.pem")),
        "{}",
        reverted.new_text
    );
    assert!(
        reverted.new_text.contains("target: 443"),
        "{}",
        reverted.new_text
    );
    assert!(
        reverted
            .plan
            .impacts
            .iter()
            .any(|i| i.code == "locked-kept"),
        "{:?}",
        reverted.plan.impacts
    );
}
