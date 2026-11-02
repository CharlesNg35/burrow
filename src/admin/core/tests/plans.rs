#![cfg(test)]

use burrow_admin_proto::msg::{AdminLevel, Apply, ErrorCode};
use burrow_admin_proto::ops::Op;
use serde_json::json;

use crate::admin::core::plan;
use crate::admin::core::*;

use super::one;
use super::{core_for, p, session, world};

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
    assert!(
        codes.contains(&burrow_admin_proto::codes::RENAMES),
        "{codes:?}"
    );
    assert!(
        !codes
            .iter()
            .any(|c| c.starts_with("removes-") || c.starts_with("adds-")),
        "a rename is not a removal plus an addition: {codes:?}"
    );
    assert!(c.plan.confirms().is_empty(), "nothing to type for a rename");
    assert!(
        c.new_docs
            .main()
            .text
            .contains("    reach:\n      home2: any # first\n      office: any\n"),
        "{}",
        c.new_docs.main().text
    );
    assert!(
        c.new_docs
            .main()
            .text
            .contains("  # the home box\n  home2:\n"),
        "{}",
        c.new_docs.main().text
    );
    let changed: Vec<(&str, &str)> = text
        .lines()
        .zip(c.new_docs.main().text.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(text.lines().count(), c.new_docs.main().text.lines().count());
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
    assert!(
        c.new_docs
            .main()
            .text
            .starts_with("# my server\nlisten: 7835 # tcp\n")
    );
    assert!(
        c.new_docs
            .main()
            .text
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
    assert!(
        c.plan
            .impacts
            .iter()
            .any(|i| i.code == burrow_admin_proto::codes::ADDS_SERVICE)
    );
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
fn a_udp_service_with_neither_bind_nor_visitors_shows_its_error_at_visitors_like_tcp() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&w.text);
    let ops = [Op::Add {
        path: p(&["services", "dns"]),
        value: json!({"client": "home", "protocol": "udp", "target": 53}),
    }];
    let c = plan::compute(&core, &s, &base, &ops).unwrap();
    let d = c
        .plan
        .diags
        .iter()
        .find(|d| d.code == "unreachable-service")
        .expect("a udp service nobody can reach is an error");
    assert!(d.concerns(&p(&["services", "dns", "visitors"])), "{d:?}");
    let ops = [Op::Add {
        path: p(&["services", "dns"]),
        value: json!({"client": "home", "protocol": "udp", "target": 53, "visitors": ["ops"]}),
    }];
    let c = plan::compute(&core, &s, &base, &ops).unwrap();
    assert!(
        c.plan
            .diags
            .iter()
            .all(|d| d.severity != burrow_admin_proto::diag::Severity::Error),
        "a private udp service with visitors is valid: {:?}",
        c.plan.diags
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
fn a_hand_edit_after_the_state_was_read_is_described_in_the_conflict() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let base = core.state(&s).unwrap().rev;
    let edited = w.text.replace("target: 3000", "target: 3001");
    core.store.write(&one(&w.text), &one(&edited)).unwrap();
    let e = plan::compute(&core, &s, &base, &[]).err().unwrap();
    assert_eq!(e.code, ErrorCode::Conflict);
    let c: burrow_admin_proto::plan::Conflict = serde_json::from_value(e.body).unwrap();
    assert!(c.known);
    let theirs: Vec<&str> = c.theirs.iter().map(|t| t.summary.as_str()).collect();
    assert_eq!(theirs, ["Service web: Main target 3000 → 3001"]);
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
                address: None,
            },
        )
        .err()
        .unwrap();
        assert_eq!(e.code, ErrorCode::LockedOut);
    }
    assert_eq!(core.read().unwrap(), w.text, "nothing was written");
}
