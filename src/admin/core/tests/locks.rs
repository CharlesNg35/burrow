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
    let params = rcgen::CertificateParams::new(
        ["nas.example.com", "*.example.com", "example.com"]
            .map(String::from)
            .to_vec(),
    )
    .unwrap();
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
fn every_field_locked_before_stays_locked_and_the_web_proxy_and_websocket_keep_their_files() {
    let core = core_for(&world().text);
    let now = core.schema.locked_paths();
    for old in BEFORE_P2
        .iter()
        .filter(|o| **o != ["web"] && **o != ["websocket"])
    {
        assert!(now.contains(&p(old)), "{old:?} is still locked: {now:?}");
    }
    let finer: [&[&str]; 23] = [
        &["websocket", "tls", "cert"],
        &["websocket", "tls", "key"],
        &["web", "letsencrypt", "ca"],
        &["web", "letsencrypt", "dns", "*", "cloudflare", "token"],
        &["web", "letsencrypt", "dns", "*", "rfc2136", "server"],
        &["web", "letsencrypt", "dns", "*", "rfc2136", "key_name"],
        &["web", "letsencrypt", "dns", "*", "rfc2136", "secret"],
        &["web", "letsencrypt", "dns", "*", "rfc2136", "algorithm"],
        &["web", "letsencrypt", "dns", "*", "acme_dns", "api"],
        &["web", "letsencrypt", "dns", "*", "acme_dns", "username"],
        &["web", "letsencrypt", "dns", "*", "acme_dns", "password"],
        &["web", "letsencrypt", "dns", "*", "command"],
        &["web", "certificates", "*", "cert"],
        &["web", "certificates", "*", "key"],
        &["web", "proxy_protocol_from"],
        &["web", "access_log"],
        &["web", "error_pages"],
        &["services", "*", "web", "tls", "cert"],
        &["services", "*", "web", "tls", "key"],
        &["services", "*", "web", "target_tls", "ca"],
        &["services", "*", "web", "client_auth", "ca"],
        &["services", "*", "web", "client_auth", "crl"],
        &["services", "*", "web", "error_pages"],
    ];
    for f in finer {
        assert!(now.contains(&p(f)), "{f:?}: {now:?}");
    }
    assert_eq!(now.len(), BEFORE_P2.len() - 2 + finer.len(), "{now:?}");
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
        json!(site.file("b.pem")),
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
            .any(|i| i.code == burrow_admin_proto::codes::LOCKED_KEPT),
        "{:?}",
        reverted.plan.impacts
    );
}

#[test]
fn the_web_proxy_and_its_ports_are_open_to_edits_except_its_files() {
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
    for ops in [
        set(&["web", "letsencrypt", "ca"], json!("certs/ca.pem")),
        set(
            &["web", "certificates", "0", "cert"],
            json!(site.file("b.pem")),
        ),
        set(
            &["web", "certificates"],
            json!([{"hosts": ["*.example.com"], "cert": site.file("b.pem"), "key": site.file("b.key")}]),
        ),
    ] {
        refused(&core, &s, &ops);
    }
    let base = text::rev(&core.read().unwrap());
    for ops in [
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

#[test]
fn certificate_entries_are_compared_by_their_own_files_not_by_place() {
    let (site, text, list) = two_entries();
    let core = core_for(&text);
    let s = session(&site.w, AdminLevel::Write);
    let base = text::rev(&core.read().unwrap());
    let set = |v: serde_json::Value| {
        vec![Op::Set {
            path: p(&["web", "certificates"]),
            value: v,
        }]
    };
    let fine = [set(json!([list[1], list[0]])), set(json!([list[1]]))];
    for ops in fine {
        let c = plan::compute(&core, &s, &base, &ops);
        assert!(c.is_ok(), "{ops:?}: {:?}", c.err().map(|e| e.message));
    }
    let mut moved = list.clone();
    moved[1]["cert"] = json!(site.file("b.pem"));
    let mut mixed = list.clone();
    mixed[1]["key"] = json!(site.file("a.key"));
    let mut twice = list.clone();
    twice[1] = list[0].clone();
    for (ops, at) in [
        (set(moved), "web.certificates.1.cert"),
        (set(mixed), "web.certificates.1.cert"),
        (set(twice), "web.certificates.0.cert"),
    ] {
        assert_eq!(refused(&core, &s, &ops), vec![at.to_string()], "{ops:?}");
    }
    let mut swapped = list.clone();
    for k in ["cert", "key"] {
        swapped[0][k] = list[1][k].clone();
        swapped[1][k] = list[0][k].clone();
    }
    let c = plan::compute(&core, &s, &base, &set(swapped)).unwrap();
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

#[test]
fn a_short_web_form_cannot_be_turned_into_one_with_files() {
    let site = site();
    let files = format!(
        "    web:\n      hosts: [nas.example.com]\n      tls: {{cert: {}, key: {}}}\n",
        site.file("a.pem"),
        site.file("a.key")
    );
    assert!(site.w.text.contains(&files));
    let with_files = json!({"hosts": ["nas.example.com"], "tls": {"cert": site.file("b.pem"), "key": site.file("b.key")}});
    for short in ["    web: [nas.example.com]\n", "    web: nas.example.com\n"] {
        let core = core_for(&site.w.text.replace(&files, short));
        let s = session(&site.w, AdminLevel::Write);
        let ops = [Op::Set {
            path: p(&["services", "nas", "web"]),
            value: with_files.clone(),
        }];
        let what = refused(&core, &s, &ops);
        assert!(what[0].contains("nas.web.tls."), "{what:?} for {short:?}");
    }
    let core = core_for(&site.w.text);
    let s = session(&site.w, AdminLevel::Write);
    let base = text::rev(&core.read().unwrap());
    let back = [Op::Set {
        path: p(&["services", "nas", "web"]),
        value: json!(["nas.example.com"]),
    }];
    let c = plan::compute(&core, &s, &base, &back);
    assert!(c.is_ok(), "{:?}", c.err().map(|e| e.message));
}

#[test]
fn a_certificate_list_added_from_nothing_is_refused_and_removing_it_passes() {
    let (site, text, list) = two_entries();
    let core = core_for(&site.w.text);
    let s = session(&site.w, AdminLevel::Write);
    let ops = [Op::Set {
        path: p(&["web", "certificates"]),
        value: json!([list[0]]),
    }];
    assert_eq!(
        refused(&core, &s, &ops),
        vec!["web.certificates.0.cert".to_string()]
    );
    let core = core_for(&text);
    let base = text::rev(&core.read().unwrap());
    let ops = [Op::Remove {
        path: p(&["web", "certificates"]),
    }];
    let c = plan::compute(&core, &s, &base, &ops);
    assert!(c.is_ok(), "{:?}", c.err().map(|e| e.message));
}
