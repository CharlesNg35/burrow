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
    files_with(main, text, drop_ins, |_| None)
}

fn files_with(
    main: &str,
    text: &str,
    drop_ins: &[(&str, &str)],
    env: fn(&str) -> Option<String>,
) -> Files {
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
        Box::new(env),
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

#[test]
fn a_short_password_blanks_no_word_in_a_text_plan_or_a_form_plan() {
    let w = world();
    let text = format!(
        "{}{}",
        w.text,
        ACME.replace("{USER}", "dns").replace("{PASS}", "pass")
    );
    let f = files("server.yaml", &text, &[]);
    let by_text = f.plan(&edit(&f, "subdomain: s1", "subdomain: s2"));
    let by_form = f.plan(&[Op::Set {
        path: p(&["visitors", "ops", "admin"]),
        value: json!("read"),
    }]);
    for plan in [by_text, by_form] {
        let sent = serde_json::to_string(&plan).unwrap();
        assert!(!sent.contains("«hidden»"), "{sent}");
        let lines: Vec<&str> = plan.diff.unified.lines().collect();
        assert!(
            lines.iter().any(|l| l.contains("password: «secret "))
                || !plan.diff.unified.contains("acme_dns"),
            "{}",
            plan.diff.unified
        );
        for d in &plan.diags {
            for text in [Some(&d.message), d.help.as_ref(), d.label.as_ref()]
                .into_iter()
                .flatten()
            {
                assert!(!text.contains("«hidden»"), "{text}");
            }
        }
    }
}

#[cfg(feature = "web")]
const SITE: &str = "web: { https: 443, http: 80, letsencrypt: { email: ops@example.com } }\n";

#[cfg(feature = "web")]
fn site(name: &str) -> String {
    format!(
        "  {name}:\n    client: home\n    bind: 8005\n    target: 3621\n    web:\n      hosts: [{name}.example.com]\n      request_headers:\n        Authorization: Basic weak-header-value-1\n"
    )
}

#[cfg(feature = "web")]
fn shows_changed(diff: &str, word: &str) -> bool {
    diff.lines()
        .any(|l| (l.starts_with('-') || l.starts_with('+')) && l.contains(word))
}

#[cfg(feature = "web")]
#[test]
fn a_renamed_items_hidden_value_reads_unchanged_in_the_history_and_in_a_revert() {
    let w = world();
    let text = w
        .text
        .replace("services:\n", &format!("{SITE}services:\n{}", site("site")));
    let f = files("server.yaml", &text, &[]);
    let base = text::docs_rev(&f.core.docs().unwrap());
    f.apply(&[Op::Rename {
        path: p(&["services", "site"]),
        to: "site2".into(),
        refs: true,
    }]);
    let h: serde_json::Value = serde_json::from_str(&f.history()).unwrap();
    let diff = h["diff"]["unified"].as_str().unwrap();
    assert!(shows_changed(diff, "site2"), "{diff}");
    assert!(!shows_changed(diff, "Authorization"), "{diff}");
    let back = f.plan(&[Op::ReplaceDocument { from_history: base }]);
    assert!(
        shows_changed(&back.diff.unified, "site2"),
        "{}",
        back.diff.unified
    );
    assert!(
        !shows_changed(&back.diff.unified, "Authorization"),
        "{}",
        back.diff.unified
    );
}

#[cfg(feature = "web")]
const URL_WEB: &str = "web:\n  https: 443\n  http: 80\n  # was https://acme:commentpw33@ca.old/dir\n  letsencrypt:\n    email: ops@example.com\n    directory: {DIR}\n";

#[cfg(feature = "web")]
const URL_PASSWORDS: [&str; 7] = [
    "literalpw22",
    "resolvedpw66",
    "ss55word",
    "literalpw44",
    "commentpw33",
    "headerpw55",
    "acme:pa",
];

#[cfg(feature = "web")]
fn url_env(name: &str) -> Option<String> {
    match name {
        "CA_HOST" => Some("ca.internal".into()),
        "ACME_PW" => Some("resolvedpw66".into()),
        _ => None,
    }
}

#[cfg(feature = "web")]
fn url_files(directory: &str) -> Files {
    let site = site("site").replace(
        "        Authorization:",
        "        X-Note: \"see https://u:headerpw55@h.example/x\"\n        Authorization:",
    );
    let text = world().text.replace(
        "services:\n",
        &format!("{}services:\n{site}", URL_WEB.replace("{DIR}", directory)),
    );
    files_with("server.yaml", &text, &[], url_env)
}

#[cfg(feature = "web")]
#[test]
fn a_password_in_a_url_never_leaves_the_server() {
    let mut applied = 0;
    for directory in [
        "https://acme:literalpw22@${CA_HOST}/dir",
        "https://acme:${ACME_PW}@ca.internal/dir",
        "https://acme:pa/ss55word@ca.internal/dir",
        "http://acme:literalpw44@ca.internal/dir",
    ] {
        let f = url_files(directory);
        let s = f.session();
        let ask = |t: &str, body: serde_json::Value| dispatch(&f.core, &s, t, &body).unwrap();
        let state = ask(burrow_admin_proto::msg::req::STATE, json!({}));
        let shown = edit_text(&f.core, &s);
        assert!(
            shown.files[0].text.contains("# was https://«secret "),
            "{directory}: {}",
            shown.files[0].text
        );
        let plan = f.plan(&edit(
            &f,
            "email: ops@example.com",
            "email: ops2@example.com",
        ));
        assert!(
            !plan.diff.unified.contains("-  # was"),
            "{}",
            plan.diff.unified
        );
        let mut sent = vec![
            ask(burrow_admin_proto::msg::req::CONFIG_VIEW, json!({})).to_string(),
            ask(
                burrow_admin_proto::msg::req::CONFIG_VIEW,
                json!({"show": "with_defaults"}),
            )
            .to_string(),
            state.to_string(),
            serde_json::to_string(&shown).unwrap(),
            serde_json::to_string(&plan).unwrap(),
        ];
        if plan.valid {
            f.apply(&edit(
                &f,
                "email: ops@example.com",
                "email: ops2@example.com",
            ));
            sent.push(f.history());
            applied += 1;
        }
        for sent in &sent {
            never(sent, &URL_PASSWORDS);
        }
        let diags = state["diags"].to_string() + &state["running"]["file_diags"].to_string();
        if directory.starts_with("http:") {
            assert!(diags.contains("http://«hidden»@ca.internal/dir"), "{diags}");
        }
    }
    assert_eq!(applied, 3);
}

#[cfg(feature = "web")]
#[test]
fn a_password_in_a_url_never_shows_in_the_web_summary_or_in_validate() {
    let text = |directory: &str| {
        world().text.replace(
            "services:\n",
            &format!("{}services:\n", URL_WEB.replace("{DIR}", directory)),
        )
    };
    let settings = super::settings_of(&text("https://acme:literalpw77@ca.internal/dir"));
    let summary = crate::config::web_summary::summary(&settings).unwrap();
    for sent in [
        serde_json::to_string(&summary).unwrap(),
        crate::config::web_summary::text(&summary),
    ] {
        never(&sent, &["literalpw77"]);
        assert!(sent.contains("https://«hidden»@ca.internal/dir"), "{sent}");
    }
    let dir = tempfile::tempdir().unwrap();
    for (directory, pw) in [
        ("https://acme:literalpw77@ca.internal/dir", "literalpw77"),
        ("http://acme:literalpw44@ca.internal/dir", "literalpw44"),
        ("https://acme:pa/ss55word@ca.internal/dir", "ss55word"),
    ] {
        let path = dir.path().join("server.yaml");
        crate::base::fs::write_private(&path, &text(directory)).unwrap();
        let nothing = |_: &str| None;
        let v = crate::config::validate::validate(&path, None, false, &nothing).unwrap();
        let style = crate::base::style::Style { color: false };
        for sent in [
            crate::config::validate::render_text(&v.outcome, false, style),
            crate::config::validate::render_json(&v.outcome, false),
        ] {
            never(&sent, &[pw, "commentpw33"]);
        }
    }
}
