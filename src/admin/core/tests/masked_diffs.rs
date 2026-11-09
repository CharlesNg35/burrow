#![cfg(test)]

use std::sync::{Arc, Mutex};

use burrow_admin_proto::features::CONFIG_FILES;
use burrow_admin_proto::msg::{AdminLevel, Apply, HistoryRequest};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::Plan;
use serde_json::json;

use super::text_edit::{edit_text, replace};
use super::{FakeLive, World, p, session, world};
use crate::admin::adapters::FileStore;
use crate::admin::core::plan::{self, ReloadResult};
use crate::admin::core::*;

struct Files {
    _dir: tempfile::TempDir,
    core: Core,
    w: World,
}

fn files(main: &str, text: &str, drop_ins: &[(&str, &str)]) -> Files {
    let w = world();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(main);
    crate::base::fs::write_private(&path, text).unwrap();
    let folder = dir.path().join(burrow_config::drop_ins::folder_name(main));
    std::fs::create_dir(&folder).unwrap();
    for (name, t) in drop_ins {
        crate::base::fs::write_private(&folder.join(name), t).unwrap();
    }
    let live = FakeLive {
        text: Arc::new(Mutex::new(w.text.clone())),
        status: Mutex::new(Default::default()),
        sessions: Mutex::new(Vec::new()),
        busy: Vec::new(),
        allows: Default::default(),
    };
    let core = Core::new(
        Box::new(FileStore::new(&path)),
        Box::new(live),
        Box::new(|_: &str| None),
    );
    Files { _dir: dir, core, w }
}

impl Files {
    fn session(&self) -> Session {
        let s = session(&self.w, AdminLevel::Write);
        s.hello(None, vec![CONFIG_FILES.to_string()]);
        s
    }

    fn plan(&self, ops: &[Op]) -> Plan {
        let base = text::docs_rev(&self.core.docs().unwrap());
        plan::compute(&self.core, &self.session(), &base, ops)
            .unwrap()
            .plan
    }

    fn apply(&self, ops: &[Op]) {
        let s = self.session();
        let base = text::docs_rev(&self.core.docs().unwrap());
        let plan = plan::compute(&self.core, &s, &base, ops).unwrap().plan;
        assert!(plan.valid, "{:#?}", plan.diags);
        let begun = plan::apply_begin(
            &self.core,
            &s,
            &Apply {
                base,
                ops: ops.to_vec(),
                plan: plan.plan.clone(),
                confirm: plan.confirms().iter().map(|c| c.to_string()).collect(),
                address: None,
            },
        )
        .unwrap();
        plan::finish(&self.core, &s, begun, ReloadResult::unchanged(), None).unwrap();
    }

    fn history(&self) -> String {
        let h = self
            .core
            .history(&HistoryRequest {
                limit: None,
                diff: None,
                entry: None,
            })
            .unwrap();
        let d = self
            .core
            .history(&HistoryRequest {
                limit: None,
                diff: None,
                entry: Some(h.entries[0].id),
            })
            .unwrap();
        assert!(d.diff.is_some());
        serde_json::to_string(&d).unwrap()
    }
}

fn edit(f: &Files, from: &str, to: &str) -> Vec<Op> {
    let t = edit_text(&f.core, &f.session());
    assert!(
        t.files.iter().any(|x| x.text.contains(from)),
        "{from} not in the text"
    );
    replace(&t, |x| x.replace(from, to))
}

fn never(sent: &str, hidden: &[&str]) {
    for h in hidden {
        assert!(!sent.contains(h), "{h} shows in {sent}");
    }
}

const ACME: &str = "web:\n  letsencrypt:\n    email: ops@example.com\n    dns:\n      - domains: [example.dev]\n        acme_dns:\n          api: https://auth.example.dev\n          username: {USER}\n          password: {PASS}\n          subdomain: s1\n";

#[test]
fn a_secret_written_with_escapes_never_shows_in_a_diff() {
    let world_text = world().text;
    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "\\\\x escape",
            "\"Gr\\xfcne-Wiese-Sonne\"".into(),
            vec!["Grüne-Wiese-Sonne", "Gr\\xfcne-Wiese-Sonne", "Wiese-Sonne"],
        ),
        (
            "\\\\u escape",
            "\"Gr\\u00fcne-Wiese-Sonne\"".into(),
            vec![
                "Grüne-Wiese-Sonne",
                "Gr\\u00fcne-Wiese-Sonne",
                "Wiese-Sonne",
            ],
        ),
        (
            "a line continuation",
            "\"tsig-part-one\\\n            -part-two\"".into(),
            vec!["tsig-part-one", "-part-two"],
        ),
        (
            "\\\\/ escape",
            "\"dHNp\\/Z2stc2VjcmV0LXZhbHVl\"".into(),
            vec![
                "dHNp/Z2stc2VjcmV0LXZhbHVl",
                "dHNp\\/Z2stc2VjcmV0LXZhbHVl",
                "Z2stc2VjcmV0LXZhbHVl",
            ],
        ),
    ];
    for (what, pass, hidden) in cases {
        let text = format!(
            "{world_text}{}",
            ACME.replace("{USER}", "u1").replace("{PASS}", &pass)
        );
        let f = files("server.yaml", &text, &[]);
        let plan = f.plan(&edit(&f, "subdomain: s1", "subdomain: s2"));
        let sent = serde_json::to_string(&plan).unwrap();
        assert!(sent.contains("subdomain: s2"), "{what}: {sent}");
        never(&sent, &hidden);
    }
}

#[test]
fn a_secret_in_a_drop_in_or_under_a_misspelled_key_never_shows_in_a_diff() {
    let w = world();
    for (what, written, hidden) in [
        (
            "an escape",
            "\"hun\\x74er2-w\\u00e9ak-secret\"",
            vec![
                "hunter2-wéak-secret",
                "w\\u00e9ak-secret",
                "x74er2",
                "u00e9ak",
            ],
        ),
        ("plain", "plain-weak-secret-1", vec!["plain-weak-secret-1"]),
    ] {
        let drop_in = format!("clients:\n  lab:\n    secret: {written}\n    max_connections: 5\n");
        let f = files("server.yaml", &w.text, &[("lab.yaml", &drop_in)]);
        let plan = f.plan(&edit(&f, "max_connections: 5", "max_connections: 6"));
        let sent = serde_json::to_string(&plan).unwrap();
        assert!(sent.contains("max_connections: 6"), "{what}: {sent}");
        never(&sent, &hidden);
    }
    let typo = "clients:\n  lab:\n    secert: typo-key-secret-9\n    max_connections: 5\n";
    let f = files("server.yaml", &w.text, &[("lab.yaml", typo)]);
    let plan = f.plan(&edit(&f, "max_connections: 5", "max_connections: 6"));
    let sent = serde_json::to_string(&plan).unwrap();
    assert!(sent.contains("max_connections: 6"), "{sent}");
    never(&sent, &["typo-key-secret-9"]);
}

fn json_escaped(s: &str) -> String {
    s.chars()
        .enumerate()
        .map(|(i, c)| match (i, c) {
            (_, '/') => "\\/".to_string(),
            (0, c) => format!("\\u{:04x}", c as u32),
            (_, c) => c.to_string(),
        })
        .collect()
}

#[test]
fn a_json_main_file_with_escaped_secrets_never_shows_them_in_a_diff_or_the_history() {
    let w = world();
    let (home, ops) = (w.home.encode(), w.ops.encode());
    let text = format!(
        "{{\n  \"listen\": 7835,\n  \"services\": {{\n    \"web\": {{ \"client\": \"home\", \"bind\": 7840, \"target\": 3000 }}\n  }},\n  \"clients\": {{\n    \"home\": {{ \"secret\": \"{}\" }}\n  }},\n  \"visitors\": {{\n    \"ops\": {{ \"secret\": \"{}\", \"admin\": \"write\" }}\n  }}\n}}\n",
        json_escaped(&home),
        json_escaped(&ops)
    );
    let f = files("server.json", &text, &[]);
    let ops_list = edit(&f, "\"target\": 3000", "\"target\": 3001");
    let plan = f.plan(&ops_list);
    let sent = serde_json::to_string(&plan).unwrap();
    assert!(sent.contains("3001"), "{sent}");
    let forms = [
        home.clone(),
        json_escaped(&home),
        home[1..12].to_string(),
        ops.clone(),
        json_escaped(&ops),
        ops[1..12].to_string(),
    ];
    let hidden: Vec<&str> = forms.iter().map(String::as_str).collect();
    never(&sent, &hidden);
    f.apply(&ops_list);
    never(&f.history(), &hidden);
}

#[test]
fn a_short_hidden_value_does_not_blank_the_same_word_in_a_form_edit() {
    let w = world();
    let text = format!(
        "{}{}",
        w.text,
        ACME.replace("{USER}", "admin").replace("{PASS}", "ops")
    );
    let f = files("server.yaml", &text, &[]);
    let plan = f.plan(&[Op::Set {
        path: p(&["visitors", "ops", "admin"]),
        value: json!("read"),
    }]);
    let unified = &plan.diff.unified;
    assert!(
        unified.contains("-    admin: write") && unified.contains("+    admin: read"),
        "{unified}"
    );
    assert!(unified.contains("  ops:"), "{unified}");
}

#[test]
fn a_file_that_does_not_parse_has_its_diff_not_shown() {
    let w = world();
    let f = files("server.yaml", &w.text, &[("lab.yaml", "clients: {}\n")]);
    let plan = f.plan(&edit(
        &f,
        "clients: {}",
        "clients: [\n  secret: hunter2-raw",
    ));
    let lab = plan
        .files
        .iter()
        .find(|d| d.file == "server.d/lab.yaml")
        .expect("a diff for lab.yaml");
    assert_eq!(
        lab.diff.unified,
        "(server.d/lab.yaml does not parse; its diff is not shown)\n"
    );
    never(&serde_json::to_string(&plan).unwrap(), &["hunter2-raw"]);
}
