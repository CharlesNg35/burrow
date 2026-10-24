#![cfg(test)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use burrow_admin_proto::msg::{AdminLevel, Apply, ErrorCode};
use burrow_admin_proto::ops::Op;
use serde_json::json;

use crate::admin::core::plan;
use crate::admin::core::ports::{Live, LiveSession};
use crate::admin::core::*;
use crate::config::settings::ServerSettings;
use crate::metrics::Status;

use super::{MemStore, World, apply_now, core_for, core_running, p, session, settings_of, world};

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
            address: None,
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
