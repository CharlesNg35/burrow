#![cfg(test)]

use burrow_admin_proto::msg::AdminLevel;
#[cfg(feature = "web")]
use burrow_admin_proto::msg::HistoryRequest;
use burrow_admin_proto::ops::Op;
use serde_json::json;

use crate::admin::core::plan;
use crate::admin::core::*;
use crate::base::testkit::cert_pair;

#[cfg(feature = "web")]
use super::apply_now;
use super::{World, core_for, p, session, world};

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
    cert_pair(
        dir,
        stem,
        &["nas.example.com", "*.example.com", "example.com"],
    );
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

fn allowed(core: &Core, s: &Session, ops: &[Op]) -> plan::Computed {
    let base = text::rev(&core.read().unwrap());
    plan::compute(core, s, &base, ops).unwrap_or_else(|e| panic!("{ops:?}: {}", e.message))
}

#[test]
fn only_the_dns_command_and_the_update_opt_in_stay_locked() {
    let core = core_for(&world().text);
    let mut now = core.schema.locked_paths();
    now.sort();
    let mut want = vec![
        p(&["update", "remote"]),
        p(&["web", "letsencrypt", "dns", "*", "command"]),
    ];
    want.sort();
    assert_eq!(now, want);
}

#[test]
fn a_sites_own_certificate_files_are_set_on_every_admin_path() {
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
        let c = allowed(&core, &s, &ops);
        assert!(
            super::yaml_has(&c.new_text, &other),
            "{ops:?}: {}",
            c.new_text
        );
    }
}

#[cfg(feature = "web")]
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

#[cfg(feature = "web")]
#[test]
fn a_revert_brings_back_the_files_of_that_version() {
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
            entry: None,
        })
        .unwrap()
        .entries
        .remove(0);
    let edited = core
        .read()
        .unwrap()
        .replace(&site.file("a.pem"), &site.file("b.pem"))
        .replace(&site.file("a.key"), &site.file("b.key"));
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
    let doc = crate::config::tree::parse(&crate::config::patch::split_text(&reverted.new_text).2)
        .unwrap()
        .to_json();
    assert_eq!(
        doc["services"]["nas"]["web"]["tls"]["cert"],
        json!(site.file("a.pem")),
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
            .all(|i| i.code != burrow_admin_proto::codes::LOCKED_KEPT),
        "{:?}",
        reverted.plan.impacts
    );
}

#[cfg(feature = "web")]
#[test]
fn the_web_proxy_its_ports_and_its_files_are_open_to_edits() {
    let site = site();
    let text = site.w.text.replace(
        "web:\n  https: 8443\n",
        &format!(
            "web:\n  https: 8443\n  certificates:\n    - {{hosts: [\"*.example.com\"], cert: {}, key: {}}}\n",
            site.file("a.pem"),
            site.file("a.key")
        ),
    );
    let core = core_for(&text);
    let s = session(&site.w, AdminLevel::Write);
    let set = |path: &[&str], value: serde_json::Value| {
        vec![Op::Set {
            path: p(path),
            value,
        }]
    };
    let base = text::rev(&core.read().unwrap());
    for ops in [
        set(&["web", "letsencrypt", "ca"], json!(site.file("a.pem"))),
        set(
            &["web", "certificates"],
            json!([{"hosts": ["*.example.com"], "cert": site.file("b.pem"), "key": site.file("b.key")}]),
        ),
        set(&["web", "letsencrypt"], json!({"email": "ops@example.com"})),
        set(
            &["web", "certificates"],
            json!([{"hosts": ["*.example.com", "example.com"], "cert": site.file("a.pem"), "key": site.file("a.key")}]),
        ),
        set(&["web", "idle_timeout_secs"], json!(30)),
        set(&["web", "https"], json!(9443)),
        set(&["web", "http"], json!(8080)),
        vec![Op::Remove { path: p(&["web"]) }],
    ] {
        let c = plan::compute(&core, &s, &base, &ops);
        assert!(c.is_ok(), "{ops:?}: {:?}", c.err().map(|e| e.message));
    }
}

#[cfg(feature = "web")]
fn two_entries() -> (Site, String, serde_json::Value) {
    let site = site();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["*.other.test".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    std::fs::write(site.dir.path().join("c.pem"), cert.pem()).unwrap();
    std::fs::write(site.dir.path().join("c.key"), key.serialize_pem()).unwrap();
    let entry = |hosts: &str, stem: &str| json!({"hosts": [hosts], "cert": site.file(&format!("{stem}.pem")), "key": site.file(&format!("{stem}.key"))});
    let list = json!([entry("*.example.com", "a"), entry("*.other.test", "c")]);
    let text = site.w.text.replace(
        "web:\n  https: 8443\n",
        &format!(
            "web:\n  https: 8443\n  certificates:\n    - {{hosts: [\"*.example.com\"], cert: {}, key: {}}}\n    - {{hosts: [\"*.other.test\"], cert: {}, key: {}}}\n",
            site.file("a.pem"),
            site.file("a.key"),
            site.file("c.pem"),
            site.file("c.key")
        ),
    );
    (site, text, list)
}

#[cfg(feature = "web")]
#[test]
fn certificate_entries_take_any_files_and_validate_judges_their_hosts() {
    let (site, text, list) = two_entries();
    let core = core_for(&text);
    let s = session(&site.w, AdminLevel::Write);
    let set = |v: serde_json::Value| {
        vec![Op::Set {
            path: p(&["web", "certificates"]),
            value: v,
        }]
    };
    let mut moved = list.clone();
    moved[1]["cert"] = json!(site.file("b.pem"));
    let mut twice = list.clone();
    twice[1] = list[0].clone();
    for ops in [
        set(json!([list[1], list[0]])),
        set(json!([list[1]])),
        set(moved),
        set(twice),
    ] {
        allowed(&core, &s, &ops);
    }
    let mut swapped = list.clone();
    for k in ["cert", "key"] {
        swapped[0][k] = list[1][k].clone();
        swapped[1][k] = list[0][k].clone();
    }
    let c = allowed(&core, &s, &set(swapped));
    assert!(
        !c.plan.valid,
        "validate refuses files that do not cover their hosts"
    );
    assert!(
        c.plan.diags.iter().any(|d| d
            .path
            .as_ref()
            .and_then(|p| p.first())
            .is_some_and(|k| k == "web")),
        "{:?}",
        c.plan.diags
    );
}

#[cfg(feature = "web")]
#[test]
fn a_revert_to_another_order_keeps_each_entrys_files_with_it() {
    let (site, text, list) = two_entries();
    let core = core_for(&text);
    let s = session(&site.w, AdminLevel::Write);
    apply_now(
        &core,
        &s,
        &[Op::Set {
            path: p(&["web", "certificates"]),
            value: json!([list[1], list[0]]),
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
    let reverted = plan::compute(
        &core,
        &s,
        &text::rev(&core.read().unwrap()),
        &[Op::ReplaceDocument {
            from_history: entry.base.clone(),
        }],
    )
    .unwrap();
    let doc = crate::config::tree::parse(&crate::config::patch::split_text(&reverted.new_text).2)
        .unwrap()
        .to_json();
    assert_eq!(doc["web"]["certificates"], list, "{}", reverted.new_text);
}

#[cfg(feature = "web")]
#[test]
fn a_short_web_form_turns_into_one_with_files_and_back() {
    let site = site();
    let files = format!(
        "    web:\n      hosts: [nas.example.com]\n      tls: {{cert: {}, key: {}}}\n",
        site.file("a.pem"),
        site.file("a.key")
    );
    assert!(site.w.text.contains(&files));
    let with_files = json!({"hosts": ["nas.example.com"], "tls": {"cert": site.file("b.pem"), "key": site.file("b.key")}});
    let s = session(&site.w, AdminLevel::Write);
    for short in ["    web: [nas.example.com]\n", "    web: nas.example.com\n"] {
        let core = super::core_running(&site.w.text.replace(&files, short), &site.w.text);
        let ops = [Op::Set {
            path: p(&["services", "nas", "web"]),
            value: with_files.clone(),
        }];
        let c = allowed(&core, &s, &ops);
        assert!(
            super::yaml_has(&c.new_text, &site.file("b.pem")),
            "{}",
            c.new_text
        );
    }
    let core = core_for(&site.w.text);
    let back = [Op::Set {
        path: p(&["services", "nas", "web"]),
        value: json!(["nas.example.com"]),
    }];
    allowed(&core, &s, &back);
}
