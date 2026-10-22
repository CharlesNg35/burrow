use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use burrow_admin_proto::msg::{AdminLevel, Apply, ConnectInfo, ErrorCode, HistoryRequest};
use burrow_admin_proto::ops::Op;
use serde_json::json;

use super::plan::{self, ReloadResult};
use super::ports::{Live, LiveSession, Store, Version, WhyNot, WriteError};
use super::*;
use crate::config::settings::ServerSettings;
use crate::config::{Format, Loaded, Role, load_str};
use crate::metrics::Status;

pub struct MemStore {
    pub text: Arc<Mutex<String>>,
    pub kept: Mutex<Vec<(Version, String)>>,
    pub read_only: bool,
}

impl Store for MemStore {
    fn path(&self) -> &Path {
        Path::new("server.yaml")
    }
    fn format(&self) -> Format {
        Format::Yaml
    }
    fn read(&self) -> std::io::Result<String> {
        Ok(self.text.lock().unwrap().clone())
    }
    fn write(&self, before: &str, after: &str) -> Result<(), WriteError> {
        let mut t = self.text.lock().unwrap();
        if *t != before {
            return Err(WriteError::Changed);
        }
        *t = after.to_string();
        Ok(())
    }
    fn writable(&self) -> Result<(), WhyNot> {
        if self.read_only {
            return Err(WhyNot {
                code: "read-only-fs".into(),
                message: "read only".into(),
                hint: String::new(),
            });
        }
        Ok(())
    }
    fn keep(&self, rev: &str, text: &str) {
        let mut k = self.kept.lock().unwrap();
        if !k.iter().any(|(v, _)| v.rev == rev) {
            let at = k.len() as u64;
            k.insert(
                0,
                (
                    Version {
                        rev: rev.into(),
                        at,
                    },
                    text.into(),
                ),
            );
            k.truncate(20);
        }
    }
    fn versions(&self) -> Vec<Version> {
        self.kept
            .lock()
            .unwrap()
            .iter()
            .map(|(v, _)| v.clone())
            .collect()
    }
    fn version(&self, rev: &str) -> Option<String> {
        self.kept
            .lock()
            .unwrap()
            .iter()
            .find(|(v, _)| v.rev == rev)
            .map(|(_, t)| t.clone())
    }
    fn history_dir(&self) -> Option<PathBuf> {
        None
    }
    fn history_problem(&self) -> Option<String> {
        None
    }
}

pub struct FakeLive {
    pub text: Arc<Mutex<String>>,
    pub status: Mutex<Status>,
    pub sessions: Mutex<Vec<LiveSession>>,
}

impl Live for FakeLive {
    fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
    fn settings(&self) -> Arc<ServerSettings> {
        Arc::new(settings_of(&self.text.lock().unwrap()))
    }
    fn sessions(&self) -> Vec<LiveSession> {
        self.sessions.lock().unwrap().clone()
    }
    fn allows(&self) -> BTreeMap<String, Vec<String>> {
        BTreeMap::new()
    }
    fn free_port(&self, near: u16, _: u16, _: bool) -> Option<u16> {
        Some(near)
    }
}

pub struct World {
    pub ops: Secret,
    pub home: Secret,
    pub text: String,
}

pub fn world() -> World {
    let (ops, home) = (Secret::generate(), Secret::generate());
    let text = format!(
        "# my server\nlisten: 7835 # tcp\n\nclients:\n  # the home box\n  home:\n    secret: {}\n\nvisitors:\n  ops:\n    secret: {}\n    admin: write\n\nservices:\n  web: {{ client: home, bind: 7840, target: 3000 }}\n",
        home.encode(),
        ops.encode()
    );
    World { ops, home, text }
}

fn settings_of(text: &str) -> ServerSettings {
    let nothing = |_: &str| None;
    match load_str(
        "server.yaml",
        text,
        Format::Yaml,
        Some(Role::Server),
        &nothing,
    )
    .loaded
    {
        Some(Loaded::Server(s, _)) => *s,
        _ => panic!("loads"),
    }
}

pub fn core_for(text: &str) -> Core {
    core_running(text, text)
}

pub fn core_with_env(
    text: &str,
    running: &str,
    vars: &'static [(&'static str, &'static str)],
) -> Core {
    let mut core = core_running(text, running);
    core.env = Box::new(move |name: &str| {
        vars.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    });
    core
}

pub fn core_running(text: &str, running: &str) -> Core {
    let shared = Arc::new(Mutex::new(text.to_string()));
    let live_text = if text == running {
        shared.clone()
    } else {
        Arc::new(Mutex::new(running.to_string()))
    };
    let store = MemStore {
        text: shared.clone(),
        kept: Mutex::new(Vec::new()),
        read_only: false,
    };
    let live = FakeLive {
        text: live_text,
        status: Mutex::new(Status::default()),
        sessions: Mutex::new(Vec::new()),
    };
    Core::new(Box::new(store), Box::new(live), Box::new(|_: &str| None))
}

pub fn session(w: &World, level: AdminLevel) -> Session {
    Session::new(
        "ops".into(),
        level,
        "203.0.113.7:5000".parse().unwrap(),
        w.ops.clone(),
        false,
        "test".into(),
    )
}

fn p(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

fn apply_now(
    core: &Core,
    s: &Session,
    ops: &[Op],
    confirm: &[&str],
) -> Result<burrow_admin_proto::plan::Applied, ApiError> {
    let base = text::rev(&core.read()?);
    let plan = plan::compute(core, s, &base, ops)?.plan;
    let begun = plan::apply_begin(
        core,
        s,
        &Apply {
            base,
            ops: ops.to_vec(),
            plan: plan.plan,
            confirm: confirm.iter().map(|c| c.to_string()).collect(),
        },
    )?;
    plan::finish(
        core,
        s,
        begun,
        ReloadResult::Applied {
            restart: Vec::new(),
            unbound: Vec::new(),
        },
        None,
    )
}

#[test]
fn a_renamed_client_keeps_its_place_in_reach_and_the_file_changes_only_the_names() {
    let w = world();
    let (office, lap) = (Secret::generate(), Secret::generate());
    let text = w
        .text
        .replace(
            "\nvisitors:",
            &format!("  office:\n    secret: {}\n\nvisitors:", office.encode()),
        )
        .replace(
            "\nservices:",
            &format!(
                "  lap:\n    secret: {}\n    reach:\n      home: any # first\n      office: any\n\nservices:",
                lap.encode()
            ),
        );
    let core = core_for(&text);
    let s = session(&w, AdminLevel::Write);
    let ops = [Op::Rename {
        path: p(&["clients", "home"]),
        to: "home2".into(),
        refs: true,
    }];
    let c = plan::compute(&core, &s, &text::rev(&text), &ops).unwrap();
    assert!(c.plan.valid, "{:?}", c.plan.diags);
    let codes: Vec<&str> = c.plan.impacts.iter().map(|i| i.code.as_str()).collect();
    assert!(codes.contains(&"renames"), "{codes:?}");
    assert!(
        !codes
            .iter()
            .any(|c| c.starts_with("removes-") || c.starts_with("adds-")),
        "a rename is not a removal plus an addition: {codes:?}"
    );
    assert!(c.plan.confirms().is_empty(), "nothing to type for a rename");
    assert!(
        c.new_text
            .contains("    reach:\n      home2: any # first\n      office: any\n"),
        "{}",
        c.new_text
    );
    assert!(
        c.new_text.contains("  # the home box\n  home2:\n"),
        "{}",
        c.new_text
    );
    let changed: Vec<(&str, &str)> = text
        .lines()
        .zip(c.new_text.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(text.lines().count(), c.new_text.lines().count());
    assert!(!changed.is_empty());
    for (a, b) in changed {
        assert_eq!(
            a.replace("home", "home2"),
            b,
            "only the name changes in place"
        );
    }
}

#[test]
fn a_plan_keeps_comments_and_shows_a_masked_diff() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&w.text);
    let ops = [Op::Add {
        path: p(&["services", "ssh"]),
        value: json!({"client": "home", "bind": 2222, "target": 22}),
    }];
    let c = plan::compute(&core, &s, &base, &ops).unwrap();
    assert!(c.plan.valid, "{:?}", c.plan.diags);
    assert!(c.new_text.starts_with("# my server\nlisten: 7835 # tcp\n"));
    assert!(
        c.new_text
            .contains("  ssh:\n    client: home\n    bind: 2222\n    target: 22\n")
    );
    assert!(c.plan.diff.unified.contains("+  ssh:"));
    assert!(!c.plan.diff.unified.contains(&w.home.encode()));
    assert!(
        c.plan
            .changes
            .iter()
            .any(|ch| ch.summary == "New service ssh")
    );
    assert!(c.plan.impacts.iter().any(|i| i.code == "adds-service"));
    assert!(c.plan.refusals.is_empty());
    let again = plan::compute(&core, &s, &base, &ops).unwrap();
    assert_eq!(again.plan.plan, c.plan.plan, "plans are deterministic");
}

#[test]
fn errors_land_on_their_field() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&w.text);
    let ops = [Op::Add {
        path: p(&["services", "api"]),
        value: json!({"client": "home", "bind": 7840, "target": 3001}),
    }];
    let c = plan::compute(&core, &s, &base, &ops).unwrap();
    assert!(!c.plan.valid);
    let d = c.plan.diags.iter().find(|d| d.is_error()).unwrap();
    assert_eq!(d.code, "duplicate-bind");
    assert!(d.concerns(&p(&["services", "api", "bind"])), "{d:?}");
    assert!(d.concerns(&p(&["services", "web", "bind"])), "{d:?}");
    let private = [Op::Remove {
        path: p(&["services", "web", "bind"]),
    }];
    let c = plan::compute(&core, &s, &base, &private).unwrap();
    let d = c
        .plan
        .diags
        .iter()
        .find(|d| d.code == "unreachable-service")
        .expect("a private service without visitors is an error");
    assert!(
        d.concerns(&p(&["services", "web", "visitors"])),
        "the error shows at the visitors field: {d:?}"
    );
}

#[test]
fn a_changed_file_is_a_conflict_and_nothing_is_written() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let e = plan::compute(&core, &s, "b2s:0000000000000000", &[])
        .err()
        .unwrap();
    assert_eq!(e.code, ErrorCode::Conflict);
    assert_eq!(e.body["current"], text::rev(&w.text));
}

#[test]
fn the_guard_refuses_lockouts() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&w.text);
    let cases = [
        (
            Op::Remove {
                path: p(&["visitors", "ops", "admin"]),
            },
            "self-demoted",
        ),
        (
            Op::Remove {
                path: p(&["visitors", "ops"]),
            },
            "self-removed",
        ),
        (
            Op::Set {
                path: p(&["visitors", "ops", "admin_from"]),
                value: json!(["10.0.0.0/8"]),
            },
            "self-source",
        ),
    ];
    for (op, code) in cases {
        let c = plan::compute(&core, &s, &base, std::slice::from_ref(&op)).unwrap();
        assert!(
            c.plan.refusals.iter().any(|r| r.code == code),
            "{code}: {:?}",
            c.plan.refusals
        );
        let e = plan::apply_begin(
            &core,
            &s,
            &Apply {
                base: base.clone(),
                ops: vec![op],
                plan: c.plan.plan.clone(),
                confirm: vec!["ops".into()],
            },
        )
        .err()
        .unwrap();
        assert_eq!(e.code, ErrorCode::LockedOut);
    }
    assert_eq!(core.read().unwrap(), w.text, "nothing was written");
}

#[test]
fn secrets_are_revealed_once_and_destructive_changes_need_confirmation() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let g = core.generate_secret(&s).unwrap();
    let applied = apply_now(
        &core,
        &s,
        &[
            Op::Add {
                path: p(&["clients", "office"]),
                value: json!({}),
            },
            Op::SecretAdd {
                path: p(&["clients", "office", "secret"]),
                handle: g.handle.clone(),
            },
        ],
        &[],
    )
    .unwrap();
    assert_eq!(applied.secrets.len(), 1);
    assert_eq!(applied.secrets[0].fp, g.fp);
    assert!(Secret::parse(&applied.secrets[0].value).is_ok());
    assert!(
        s.pending_secret(&g.handle).is_none(),
        "the handle is used up"
    );
    let e = apply_now(
        &core,
        &s,
        &[Op::Remove {
            path: p(&["clients", "office"]),
        }],
        &[],
    )
    .err()
    .unwrap();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert_eq!(e.body["confirm"], json!(["office"]));
    apply_now(
        &core,
        &s,
        &[Op::Remove {
            path: p(&["clients", "office"]),
        }],
        &["office"],
    )
    .unwrap();
    let h = core
        .history(&HistoryRequest {
            limit: None,
            diff: None,
        })
        .unwrap();
    assert_eq!(h.entries.len(), 2);
    assert!(h.entries[0].changes[0].summary.contains("office removed"));
    assert!(
        !serde_json::to_string(&h)
            .unwrap()
            .contains(&applied.secrets[0].value)
    );
}

#[test]
fn a_new_client_comes_with_ready_snippets_and_later_only_with_a_placeholder() {
    let w = world();
    let core = core_for(&w.text.replace(
        "listen: 7835 # tcp\n",
        "listen: 7835 # tcp\npublic_address: tunnel.example.com\n",
    ));
    let s = session(&w, AdminLevel::Write);
    let g = core.generate_secret(&s).unwrap();
    let applied = apply_now(
        &core,
        &s,
        &[
            Op::Add {
                path: p(&["clients", "office"]),
                value: json!({}),
            },
            Op::SecretAdd {
                path: p(&["clients", "office", "secret"]),
                handle: g.handle.clone(),
            },
            Op::Add {
                path: p(&["services", "db"]),
                value: json!({"client": "office", "target": "10.0.0.5:5432", "bind": 7841}),
            },
        ],
        &[],
    )
    .unwrap();
    let value = applied.secrets[0].value.clone();
    assert_eq!(applied.snippets.len(), 1);
    let sn = &applied.snippets[0];
    assert_eq!((sn.kind.as_str(), sn.name.as_str()), ("client", "office"));
    assert!(sn.server_known);
    assert_eq!(sn.server, "tunnel.example.com:7835");
    assert!(sn.with_secret);
    assert_eq!(sn.secret_fp.as_deref(), Some(g.fp.as_str()));
    assert_eq!(
        sn.config,
        format!(
            "server: tunnel.example.com:7835\nname: office\nsecret: {value}\nallow:\n  - 10.0.0.5/32:5432\n"
        )
    );
    assert!(
        sn.env_config.contains("secret: \"${OFFICE_SECRET}\""),
        "{}",
        sn.env_config
    );
    assert!(!sn.env_config.contains(&value));
    assert!(sn.commands.iter().any(|c| c.os == "linux"
        && c.line == "burrow init client --server tunnel.example.com:7835 --name office"));
    assert!(
        sn.commands
            .iter()
            .any(|c| c.os == "windows" && c.line == "burrow service install -c client.yaml")
    );

    let later = snippets::connect_info(
        &core,
        &s,
        &ConnectInfo {
            kind: "client".into(),
            name: "office".into(),
            address: None,
        },
    )
    .unwrap();
    assert!(!later.with_secret);
    assert!(
        !later.config.contains(&value),
        "a later look never shows the secret"
    );
    assert!(later.config.contains("${OFFICE_SECRET}"));
    let unknown = snippets::connect_info(
        &core,
        &s,
        &ConnectInfo {
            kind: "visitor".into(),
            name: "nobody".into(),
            address: Some("other.example".into()),
        },
    );
    assert!(unknown.is_err());
    let none = core_for(&w.text);
    let placeholder = snippets::connect_info(
        &none,
        &s,
        &ConnectInfo {
            kind: "client".into(),
            name: "home".into(),
            address: None,
        },
    )
    .unwrap();
    assert!(!placeholder.server_known);
    assert!(placeholder.config.contains("server: SERVER_ADDRESS"));
}

#[test]
fn read_level_and_read_only_files_cannot_write() {
    let w = world();
    let core = core_for(&w.text);
    let r = session(&w, AdminLevel::Read);
    let e = dispatch(
        &core,
        &r,
        "validate",
        &json!({"base": text::rev(&w.text), "ops": []}),
    )
    .err()
    .unwrap();
    assert_eq!(e.code, ErrorCode::Forbidden);
    assert!(core.generate_secret(&r).is_err());
    let st = core.state(&r).unwrap();
    assert!(!st.file.writable);
    assert_eq!(st.file.why_not.unwrap().code, "read-level");
    assert!(st.values["visitors"]["ops"]["secret"]["$secret"]["fp"].is_string());
    assert!(
        !serde_json::to_string(&st.values)
            .unwrap()
            .contains(&w.ops.encode())
    );
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
            diff: Some(entry.rev.clone()),
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
            diff: Some(entry.rev.clone()),
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
        (p(&["websocket"]), json!({"listen": 7837})),
        (p(&["websocket", "path"]), json!("/x")),
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
            text: "listen: 1\n".into(),
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
            p(&["websocket"]),
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
        .find(|i| i.code == "locked-kept")
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
        .find(|d| d.code == "listen-clash")
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
            text: "max_pending: 9\n".into(),
        }],
    )
    .err()
    .unwrap();
    assert_eq!(
        locked.code,
        ErrorCode::LockedField,
        "a block with locked fields inside is locked"
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
        })
        .unwrap();
    assert_eq!(h.entries[0].result, "rejected, restored");
}

#[test]
fn generated_secrets_are_capped_per_session() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    for _ in 0..MAX_PENDING_SECRETS {
        core.generate_secret(&s).unwrap();
    }
    assert_eq!(
        core.generate_secret(&s).err().unwrap().code,
        ErrorCode::Busy
    );
}

struct Busy {
    text: String,
    status: Arc<Mutex<Status>>,
}

impl Live for Busy {
    fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
    fn settings(&self) -> Arc<ServerSettings> {
        Arc::new(settings_of(&self.text))
    }
    fn sessions(&self) -> Vec<LiveSession> {
        Vec::new()
    }
    fn allows(&self) -> BTreeMap<String, Vec<String>> {
        BTreeMap::new()
    }
    fn free_port(&self, near: u16, _: u16, _: bool) -> Option<u16> {
        Some(near)
    }
}

#[test]
fn a_changing_connection_count_does_not_invalidate_a_reviewed_plan() {
    let w = world();
    let status = Arc::new(Mutex::new(Status::default()));
    let open = |n: u64| {
        serde_json::from_value::<Status>(json!({"services": [{"name": "web", "open": n}]})).unwrap()
    };
    let store = MemStore {
        text: Arc::new(Mutex::new(w.text.clone())),
        kept: Mutex::new(Vec::new()),
        read_only: false,
    };
    let core = Core::new(
        Box::new(store),
        Box::new(Busy {
            text: w.text.clone(),
            status: status.clone(),
        }),
        Box::new(|_: &str| None),
    );
    let s = session(&w, AdminLevel::Write);
    let ops = [Op::Remove {
        path: p(&["services", "web"]),
    }];
    let base = text::rev(&core.read().unwrap());
    let hash = |n: u64| {
        *status.lock().unwrap() = open(n);
        let plan = plan::compute(&core, &s, &base, &ops).unwrap().plan;
        (plan.plan.clone(), plan.impacts[0].message.clone())
    };
    let (three, says_three) = hash(3);
    let (five, says_five) = hash(5);
    assert!(says_three.contains("3 open connections") && says_five.contains("5 open connections"));
    assert_eq!(three, five, "only the count changed");
    assert_eq!(hash(1).0, three, "one connection is still a busy service");
    assert_ne!(hash(0).0, three, "a service that became idle is a new plan");
    *status.lock().unwrap() = open(3);
    let reviewed = plan::compute(&core, &s, &base, &ops).unwrap().plan;
    *status.lock().unwrap() = open(7);
    let begun = plan::apply_begin(
        &core,
        &s,
        &Apply {
            base,
            ops: ops.to_vec(),
            plan: reviewed.plan,
            confirm: vec!["web".into()],
        },
    );
    assert!(begun.is_ok(), "{:?}", begun.err().map(|e| e.message));
}

#[test]
fn an_apply_adds_a_service_on_this_server() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let ops = [Op::Add {
        path: p(&["services", "local"]),
        value: json!({"client": "self", "bind": 7841, "target": 3000}),
    }];
    let applied = apply_now(&core, &s, &ops, &[]).unwrap();
    assert!(!applied.rev.is_empty());
    let text = core.read().unwrap();
    assert!(
        text.contains("  local:\n    client: self\n    bind: 7841\n    target: 3000\n"),
        "{text}"
    );
    let named = [Op::Add {
        path: p(&["clients", "self"]),
        value: json!({"secret": Secret::generate().encode()}),
    }];
    let base = text::rev(&text);
    let c = plan::compute(&core, &s, &base, &named).unwrap();
    assert!(!c.plan.valid);
    assert!(c.plan.diags.iter().any(|d| d.code == "reserved-name"));
}

struct Broken {
    w: World,
    boss: Secret,
    running: String,
}

const TYPO: &str = "  api: { client: home, bind: 7840, target: 3001 }\n";

fn broken_world() -> Broken {
    let w = world();
    let boss = Secret::generate();
    let running = w
        .text
        .replace(
            "\nservices:",
            &format!(
                "  boss:\n    secret: {}\n    admin: write\n    admin_from: [10.0.0.0/8]\n\nservices:",
                boss.encode()
            ),
        )
        .replace(
            "{ client: home, bind: 7840, target: 3000 }",
            "{ client: home, bind: 7840, target: 3000, visitors: [boss] }",
        );
    Broken { w, boss, running }
}

impl Broken {
    fn with_typo(&self, edit: impl Fn(&str) -> String) -> String {
        let t = edit(&self.running);
        t.replacen(
            "visitors: [boss] }\n",
            &format!("visitors: [boss] }}\n{TYPO}"),
            1,
        )
    }

    fn refusals(&self, file: &str, ops: &[Op]) -> Vec<burrow_admin_proto::plan::Refusal> {
        let core = core_running(file, &self.running);
        let s = session(&self.w, AdminLevel::Write);
        let c = plan::compute(&core, &s, &text::rev(file), ops).unwrap();
        assert!(c.plan.valid, "{:?}", c.plan.diags);
        let e = apply_now(&core, &s, ops, &["boss", "api"]).unwrap_err();
        assert_eq!(e.code, ErrorCode::LockedOut, "{e:?}");
        assert_eq!(core.read().unwrap(), file, "nothing was written");
        c.plan.refusals
    }
}

fn remove_typo() -> Op {
    Op::Remove {
        path: p(&["services", "api"]),
    }
}

fn file_has_errors(r: &[burrow_admin_proto::plan::Refusal], code: &str) {
    assert!(
        r.iter().any(|r| r.code == code
            && r.message.contains("the config file has errors")
            && r.message.contains("burrow edit -c server.yaml")),
        "{code}: {r:?}"
    );
}

#[test]
fn an_invalid_file_does_not_undo_a_revoked_admin() {
    let b = broken_world();
    let deleted = b.running.replace(
        &format!(
            "  boss:\n    secret: {}\n    admin: write\n    admin_from: [10.0.0.0/8]\n",
            b.boss.encode()
        ),
        "",
    );
    assert!(!plan::check_text(&core_for(&deleted), &deleted).valid);
    let r = b.refusals(
        &deleted,
        &[Op::Add {
            path: p(&["visitors", "boss"]),
            value: json!({"secret": b.boss.encode(), "admin": "write", "admin_from": ["10.0.0.0/8"]}),
        }],
    );
    file_has_errors(&r, "admin-grant");
}

#[test]
fn an_invalid_file_does_not_undo_a_lowered_level_or_a_narrowed_admin_from() {
    let b = broken_world();
    let read = b.with_typo(|t| {
        t.replacen(
            "    admin: write\n    admin_from",
            "    admin: read\n    admin_from",
            1,
        )
    });
    let r = b.refusals(
        &read,
        &[
            remove_typo(),
            Op::Set {
                path: p(&["visitors", "boss", "admin"]),
                value: json!("write"),
            },
        ],
    );
    file_has_errors(&r, "admin-upgrade");
    let narrow = b.with_typo(|t| t.replace("[10.0.0.0/8]", "[10.1.0.0/16]"));
    let r = b.refusals(
        &narrow,
        &[
            remove_typo(),
            Op::Set {
                path: p(&["visitors", "boss", "admin_from"]),
                value: json!(["10.0.0.0/8"]),
            },
        ],
    );
    file_has_errors(&r, "admin-from-widened");
}

#[test]
fn an_invalid_file_does_not_let_a_revert_bring_power_back() {
    let b = broken_world();
    let read = b.with_typo(|t| {
        t.replacen(
            "    admin: write\n    admin_from",
            "    admin: read\n    admin_from",
            1,
        )
    });
    let core = core_running(&read, &b.running);
    core.store.keep(&text::rev(&b.running), &b.running);
    let s = session(&b.w, AdminLevel::Write);
    let ops = [Op::ReplaceDocument {
        from_history: text::rev(&b.running),
    }];
    let c = plan::compute(&core, &s, &text::rev(&read), &ops).unwrap();
    assert!(c.plan.valid, "{:?}", c.plan.diags);
    file_has_errors(&c.plan.refusals, "admin-upgrade");
    assert!(apply_now(&core, &s, &ops, &["boss", "api"]).is_err());
    assert_eq!(core.read().unwrap(), read);
}

#[test]
fn a_grant_added_by_hand_to_an_invalid_file_is_left_to_the_file() {
    let b = broken_world();
    let helper = format!(
        "  helper:\n    secret: {}\n    admin: read\n  ops:\n",
        Secret::generate().encode()
    );
    let granted = b.with_typo(|t| t.replacen("  ops:\n", &helper, 1));
    let core = core_running(&granted, &b.running);
    let s = session(&b.w, AdminLevel::Write);
    let c = plan::compute(&core, &s, &text::rev(&granted), &[remove_typo()]).unwrap();
    file_has_errors(&c.plan.refusals, "admin-grant");
    let valid = b.running.replacen("  ops:\n", &helper, 1);
    let core = core_running(&valid, &b.running);
    let c = plan::compute(
        &core,
        &s,
        &text::rev(&valid),
        &[Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3002),
        }],
    )
    .unwrap();
    assert!(c.plan.refusals.is_empty(), "{:?}", c.plan.refusals);
}

#[test]
fn a_plan_tells_new_problems_from_old_ones_and_groups_references_under_their_entity() {
    let w = world();
    let lap = Secret::generate();
    let text = w.text.replace(
        "\nservices:",
        &format!(
            "  lap:\n    secret: {}\n    reach:\n      home: any\n\nservices:\n  db: {{ client: home, bind: 7841, target: 5432 }}",
            lap.encode()
        ),
    );
    let core = core_for(&text);
    let s = session(&w, AdminLevel::Write);
    let office = Secret::generate();
    let ops = [
        Op::Add {
            path: p(&["clients", "office"]),
            value: json!({"secret": office.encode()}),
        },
        Op::Remove {
            path: p(&["services", "web", "client"]),
        },
        Op::Add {
            path: p(&["services", "web", "clients"]),
            value: json!(["home", "office"]),
        },
        Op::Set {
            path: p(&["services", "db", "client"]),
            value: json!("office"),
        },
        Op::Add {
            path: p(&["visitors", "lap", "reach", "office"]),
            value: json!("any"),
        },
    ];
    let c = plan::compute(&core, &s, &text::rev(&text), &ops).unwrap();
    assert!(c.plan.valid, "{:?}", c.plan.diags);
    let old: Vec<String> = c
        .plan
        .diags
        .iter()
        .filter(|d| d.before)
        .map(|d| d.path.as_ref().map(|p| p.join(".")).unwrap_or_default())
        .collect();
    let new: Vec<String> = c
        .plan
        .diags
        .iter()
        .filter(|d| !d.before)
        .map(|d| d.path.as_ref().map(|p| p.join(".")).unwrap_or_default())
        .collect();
    assert_eq!(old, ["visitors.lap.reach.home"], "{:?}", c.plan.diags);
    assert_eq!(new, ["visitors.lap.reach.office"], "{:?}", c.plan.diags);
    let group = |path: &[&str]| {
        c.plan
            .changes
            .iter()
            .find(|ch| ch.path == p(path))
            .unwrap_or_else(|| panic!("{path:?} in {:?}", c.plan.changes))
            .part_of
            .clone()
    };
    assert_eq!(group(&["clients", "office"]), None);
    assert_eq!(group(&["services", "web"]), Some(p(&["clients", "office"])));
    assert_eq!(group(&["visitors", "lap"]), Some(p(&["clients", "office"])));
    assert_eq!(group(&["services", "db"]), None, "db moves away from home");
}
