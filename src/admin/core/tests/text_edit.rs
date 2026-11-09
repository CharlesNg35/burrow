#![cfg(test)]

use burrow_admin_proto::msg::{AdminLevel, Apply, ErrorCode, HistoryRequest, req};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::Plan;
use burrow_admin_proto::text_edit::{EditText, TextFile};
use serde_json::json;

use super::{core_for, p, session, world};
use crate::admin::core::plan::{self, ReloadResult};
use crate::admin::core::*;

pub(super) fn edit_text(core: &Core, s: &Session) -> EditText {
    serde_json::from_value(dispatch(core, s, req::CONFIG_TEXT, &json!({})).unwrap()).unwrap()
}

pub(super) fn replace(t: &EditText, f: impl Fn(&str) -> String) -> Vec<Op> {
    vec![Op::ReplaceText {
        files: t
            .files
            .iter()
            .map(|x| TextFile {
                name: x.name.clone(),
                text: f(&x.text),
            })
            .collect(),
    }]
}

fn planned(core: &Core, s: &Session, ops: &[Op]) -> Result<Plan, ApiError> {
    let base = text::docs_rev(&core.docs().unwrap());
    plan::compute(core, s, &base, ops).map(|c| c.plan)
}

fn applied(core: &Core, s: &Session, ops: &[Op]) -> Plan {
    let base = text::docs_rev(&core.docs().unwrap());
    let plan = plan::compute(core, s, &base, ops).unwrap().plan;
    assert!(plan.valid, "{:#?}", plan.diags);
    let begun = plan::apply_begin(
        core,
        s,
        &Apply {
            base,
            ops: ops.to_vec(),
            plan: plan.plan.clone(),
            confirm: plan.confirms().iter().map(|c| c.to_string()).collect(),
            address: None,
        },
    )
    .unwrap();
    plan::finish(core, s, begun, ReloadResult::unchanged(), None).unwrap();
    plan
}

#[test]
fn the_text_comes_with_the_rev_the_schema_and_the_pattern_and_any_admin_may_read_it() {
    let w = world();
    let core = core_for(&w.text);
    let t = edit_text(&core, &session(&w, AdminLevel::Read));
    assert_eq!(t.rev, text::docs_rev(&core.docs().unwrap()));
    assert_eq!(t.schema_hash, core.schema().hash);
    assert_eq!(
        t.placeholder,
        burrow_admin_proto::text_edit::PLACEHOLDER_PATTERN
    );
    assert_eq!(t.files.len(), 1);
    assert_eq!(t.files[0].name, "server.yaml");
    assert_eq!(t.files[0].read_only, None);
    assert!(t.schema["properties"]["clients"].is_object());
    let shown = t.files[0].text.clone();
    assert!(!shown.contains(&w.home.encode()) && !shown.contains(&w.ops.encode()));
    assert!(shown.contains(&format!("secret: «secret {}»", w.home.fingerprint())));
    let back = shown
        .replace(
            &format!("«secret {}»", w.home.fingerprint()),
            &w.home.encode(),
        )
        .replace(
            &format!("«secret {}»", w.ops.fingerprint()),
            &w.ops.encode(),
        );
    assert_eq!(back, w.text, "only the secrets differ");
}

#[test]
fn the_unchanged_text_is_no_change() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let c = plan::compute(&core, &s, &t.rev, &replace(&t, str::to_string)).unwrap();
    assert_eq!(c.new_docs, c.old_docs);
    assert!(c.plan.changes.is_empty(), "{:?}", c.plan.changes);
    assert!(c.plan.valid, "{:?}", c.plan.diags);
}

#[test]
fn an_edit_in_the_text_plans_like_the_same_field_change() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let by_text = planned(
        &core,
        &s,
        &replace(&t, |x| x.replace("target: 3000", "target: 3001")),
    )
    .unwrap();
    let by_field = planned(
        &core,
        &s,
        &[Op::Set {
            path: p(&["services", "web", "target"]),
            value: json!(3001),
        }],
    )
    .unwrap();
    assert!(by_text.valid, "{:?}", by_text.diags);
    assert_eq!(by_text.diff, by_field.diff);
    assert_eq!(by_text.changes, by_field.changes);
    assert_eq!(by_text.result_rev, by_field.result_rev);
    assert_eq!(by_text.applies, by_field.applies);
}

#[test]
fn a_text_edit_on_an_old_base_is_a_conflict() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let old = text::rev("listen: 1\n");
    let e = plan::compute(&core, &s, &old, &replace(&t, str::to_string))
        .err()
        .unwrap();
    assert_eq!(e.code, ErrorCode::Conflict);
}

#[test]
fn replace_text_stands_alone_and_names_only_files_burrow_can_use() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let mut ops = replace(&t, str::to_string);
    ops.push(Op::Remove {
        path: p(&["services", "web"]),
    });
    let e = planned(&core, &s, &ops).err().unwrap();
    assert_eq!(
        (e.code, e.message.as_str()),
        (
            ErrorCode::BadRequest,
            "replace_text goes alone, or with drop_file for files it leaves empty"
        )
    );
    let other = vec![Op::ReplaceText {
        files: vec![TextFile {
            name: "notes.txt".into(),
            text: "services: {}\n".into(),
        }],
    }];
    let e = planned(&core, &s, &other).err().unwrap();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert!(
        e.message
            .contains("\"notes.txt\" is not a drop-in file name; use server.d/<name>.yaml"),
        "{}",
        e.message
    );
    let mut twice = replace(&t, str::to_string);
    if let Op::ReplaceText { files } = &mut twice[0] {
        files.push(files[0].clone());
    }
    let e = planned(&core, &s, &twice).err().unwrap();
    assert!(e.message.contains("sent twice"), "{}", e.message);
    let read = dispatch(
        &core,
        &session(&w, AdminLevel::Read),
        req::VALIDATE,
        &json!({ "base": t.rev, "ops": replace(&t, str::to_string) }),
    )
    .unwrap_err();
    assert_eq!(read.code, ErrorCode::Forbidden);
}

#[test]
fn a_placeholder_moved_to_another_field_is_refused_with_where_it_belongs() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let (home, ops) = (
        format!("«secret {}»", w.home.fingerprint()),
        format!("«secret {}»", w.ops.fingerprint()),
    );
    let e = planned(&core, &s, &replace(&t, |x| x.replace(&ops, &home)))
        .err()
        .unwrap();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(
        e.message.starts_with("server.yaml line 11, visitors.ops.secret: this placeholder stands for the secret of clients.home.secret"),
        "{}",
        e.message
    );
    let diags = e.body["diags"].as_array().unwrap();
    assert_eq!(diags[0]["path"], json!(["visitors", "ops", "secret"]));
    assert_eq!(diags[0]["file"], "server.yaml");
    assert_eq!(diags[0]["line"], 11);
    assert!(!e.body.to_string().contains(&w.home.encode()));
}

#[test]
fn problems_name_their_file_and_point_into_the_text_as_shown() {
    let w = world();
    let text = format!(
        "listen: 7835\nclients:\n  home: {{ secret: {} }}\nvisitors:\n  ops: {{ secret: {}, admin: write }}\nservices:\n  web: {{ client: home, bind: 7840, target: 3000 }}\n",
        w.home.encode(),
        w.ops.encode()
    );
    let core = core_for(&text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let shown = t.files[0].text.replace("admin: write", "admin: wrong");
    let plan = planned(&core, &s, &replace(&t, |_| shown.clone())).unwrap();
    assert!(!plan.valid);
    let d = plan
        .diags
        .iter()
        .find(|d| d.path.as_deref() == Some(&p(&["visitors", "ops", "admin"])[..]))
        .unwrap_or_else(|| panic!("{:#?}", plan.diags));
    let line = shown.lines().nth(4).unwrap();
    let col = line[..line.find("wrong").unwrap()].chars().count() + 1;
    assert_eq!((d.line, d.col), (Some(5), Some(col as u32)), "{d:?}");
    assert_eq!(d.end_col, Some(col as u32 + 5), "{d:?}");
    assert!(
        plan.diags
            .iter()
            .all(|d| d.file.as_deref() == Some("server.yaml")),
        "{:#?}",
        plan.diags
    );
}

#[test]
fn a_text_that_does_not_parse_plans_with_its_syntax_error() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let plan = planned(&core, &s, &replace(&t, |x| format!("{x}listen: [\n"))).unwrap();
    assert!(!plan.valid);
    let d = plan.diags.iter().find(|d| d.is_error()).unwrap();
    assert_eq!(d.file.as_deref(), Some("server.yaml"));
    assert!(d.line.is_some(), "{d:?}");
    assert!(
        !serde_json::to_string(&plan)
            .unwrap()
            .contains(&w.home.encode())
    );
}

#[test]
fn a_locked_field_changed_in_the_text_is_refused_not_kept() {
    let w = world();
    let text = format!(
        "{}web:\n  letsencrypt:\n    email: ops@example.com\n    dns:\n      - domains: [example.com]\n        command: /usr/local/bin/dns-hook\n",
        w.text
    );
    let core = super::core_running(&text, &w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let ops = replace(&t, |x| x.replace("/usr/local/bin/dns-hook", "/tmp/other"));
    let e = planned(&core, &s, &ops).err().unwrap();
    assert_eq!(e.code, ErrorCode::LockedField, "{}", e.message);
    assert_eq!(
        e.body["path"],
        json!(["web", "letsencrypt", "dns", "0", "command"])
    );
    let fine = replace(&t, |x| {
        x.replace("email: ops@example.com", "email: it@example.com")
    });
    assert!(planned(&core, &s, &fine).is_ok());
}

#[cfg(feature = "web")]
fn placeholder_paths(
    n: &crate::config::tree::Node,
    at: &mut Vec<String>,
    out: &mut Vec<Vec<String>>,
) {
    use crate::config::tree::Value as T;
    match &n.value {
        T::Map(entries) => entries.iter().for_each(|e| {
            at.push(e.key.clone());
            placeholder_paths(&e.value, at, out);
            at.pop();
        }),
        T::Seq(items) => items.iter().enumerate().for_each(|(i, it)| {
            at.push(i.to_string());
            placeholder_paths(it, at, out);
            at.pop();
        }),
        T::Str(s) if !burrow_admin_proto::text_edit::placeholders_in(s).is_empty() => {
            out.push(at.clone())
        }
        _ => {}
    }
}

#[cfg(feature = "web")]
fn x_secret_at(schema: &serde_json::Value, path: &[String]) -> bool {
    let mut at = vec![schema];
    for key in path {
        let mut next = Vec::new();
        while let Some(js) = at.pop() {
            if js["x-secret"] == true {
                return true;
            }
            for k in ["anyOf", "oneOf", "allOf"] {
                at.extend(js[k].as_array().into_iter().flatten());
            }
            next.extend(js["properties"].get(key));
            next.extend(js["items"].as_object().map(|_| &js["items"]));
            if let Some(pp) = js["patternProperties"].as_object() {
                let re = header_entry(pp, key);
                next.extend(re);
            }
            next.extend(
                js["additionalProperties"]
                    .as_object()
                    .map(|_| &js["additionalProperties"]),
            );
        }
        at = next;
    }
    let mut stack = at;
    while let Some(js) = stack.pop() {
        if js["x-secret"] == true {
            return true;
        }
        for k in ["anyOf", "oneOf", "allOf"] {
            stack.extend(js[k].as_array().into_iter().flatten());
        }
    }
    false
}

#[cfg(feature = "web")]
fn header_entry<'a>(
    pp: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a serde_json::Value> {
    burrow_syntax::secret_fmt::credential_header(key)
        .then(|| pp.values().next())
        .flatten()
}

#[cfg(feature = "web")]
#[test]
fn every_secret_hidden_in_the_text_is_a_secret_field_of_the_editor_schema() {
    let all = super::view_secrets::every_secret();
    let laptop = crate::wire::secret::Secret::generate();
    let core = super::view_secrets::core_of(&all, &laptop);
    let t = edit_text(&core, &session(&all.world, AdminLevel::Write));
    let tree = crate::config::tree::parse(&t.files[0].text).unwrap();
    let mut paths = Vec::new();
    placeholder_paths(&tree, &mut Vec::new(), &mut paths);
    assert!(paths.len() >= 10, "{paths:?}");
    for path in paths {
        assert!(x_secret_at(&t.schema, &path), "{path:?} is not x-secret");
    }
}

#[cfg(feature = "web")]
#[test]
fn no_secret_reaches_the_text_a_plan_the_audit_or_the_history() {
    let all = super::view_secrets::every_secret();
    let laptop = crate::wire::secret::Secret::generate();
    let core = super::view_secrets::core_of(&all, &laptop);
    let never = super::view_secrets::never_shown(&all, &laptop);
    let s = session(&all.world, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let mut sent = vec![serde_json::to_string(&t).unwrap()];
    let ops = replace(&t, |x| {
        x.replace("http: 7899", "http: 7898")
            .replace("ops@example.com", "it@example.com")
    });
    let plan = planned(&core, &s, &ops).unwrap();
    sent.push(serde_json::to_string(&plan).unwrap());
    if plan.valid {
        applied(&core, &s, &ops);
        let h = core
            .history(&HistoryRequest {
                limit: None,
                diff: None,
                entry: None,
            })
            .unwrap();
        let id = h.entries[0].id;
        sent.push(serde_json::to_string(&h).unwrap());
        let d = core
            .history(&HistoryRequest {
                limit: None,
                diff: None,
                entry: Some(id),
            })
            .unwrap();
        assert!(d.diff.is_some());
        sent.push(serde_json::to_string(&d).unwrap());
    }
    for text in &sent {
        for secret in &never {
            assert!(!text.contains(secret.as_str()), "{secret} sent in {text}");
        }
    }
    assert!(plan.valid, "{:#?}", plan.diags);
}

#[test]
fn a_weak_secret_on_disk_is_masked_in_the_text_and_in_the_diff_around_it() {
    let w = world();
    let text = w.text.replace(&w.home.encode(), "hunter2-weak").replace(
        "\nvisitors:",
        "  odd:\n    secret: [w3ak-one, \"${SPARE}\"]\n\nvisitors:",
    );
    let core = super::core_running(&text, &w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    assert!(!t.files[0].text.contains("hunter2-weak") && !t.files[0].text.contains("w3ak-one"));
    let plan = planned(
        &core,
        &s,
        &replace(&t, |x| x.replace("# the home box", "# the home server")),
    )
    .unwrap();
    let sent = serde_json::to_string(&plan).unwrap();
    assert!(sent.contains("the home server"), "{sent}");
    assert!(
        !sent.contains("hunter2-weak") && !sent.contains("w3ak-one"),
        "{sent}"
    );
}

#[test]
fn an_edit_of_comments_only_says_so_in_review_and_activity() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let ops = replace(&t, |x| x.replace("# the home box", "# the box at home"));
    let plan = applied(&core, &s, &ops);
    let lines: Vec<&str> = plan.changes.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(lines, ["server.yaml: comments or layout changed"]);
    assert!(core.read().unwrap().contains("# the box at home"));
    let h = core
        .history(&HistoryRequest {
            limit: None,
            diff: None,
            entry: None,
        })
        .unwrap();
    assert_eq!(h.entries[0].changes, plan.changes);
}

#[test]
fn every_line_of_a_text_plan_is_veiled_with_the_secrets_of_both_texts() {
    let w = world();
    let text = w.text.replace(&w.home.encode(), "hunter2,LEAKC7");
    let core = super::core_running(&text, &w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let ops = replace(&t, |x| {
        x.replace(
            "admin: write",
            "admin: write\n    note_secret: typed,LEAKNEW8",
        )
    });
    let old = core.docs().unwrap();
    let e = crate::admin::core::text_edit::edited(&core, &old, &ops)
        .unwrap()
        .unwrap();
    let plan = planned(&core, &s, &ops).unwrap();
    let sent = serde_json::to_string(&plan).unwrap();
    assert!(
        !sent.contains("LEAKC7") && !sent.contains("LEAKNEW8"),
        "{sent}"
    );
    let err = e.veil_error(
        &core,
        ApiError::new(ErrorCode::Invalid, "hunter2,LEAKC7 is wrong")
            .with(json!({"diags": [{"message": "typed,LEAKNEW8"}]})),
    );
    assert!(!err.message.contains("LEAKC7") && !err.body.to_string().contains("LEAKNEW8"));
}

#[test]
fn a_secret_moved_into_a_flow_mapping_never_shows_a_part_of_itself() {
    let w = world();
    let text = w.text.replace(&w.home.encode(), "hunter2,LEAKC7");
    let core = super::core_running(&text, &w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let shown = &t.files[0].text;
    let at = shown.find("  home:\n    secret: ").unwrap();
    let line_end = at + shown[at..].find("\n    secret: ").unwrap() + 1;
    let p_end = line_end + shown[line_end..].find('\n').unwrap();
    let p = shown[line_end..p_end]
        .trim()
        .trim_start_matches("secret: ")
        .to_string();
    let sent = format!(
        "{}  home: {{ secret: {p} }}{}",
        &shown[..at],
        &shown[p_end..]
    );
    let c = plan::compute(&core, &s, &t.rev, &replace(&t, |_| sent.clone())).unwrap();
    let out = serde_json::to_string(&c.plan).unwrap();
    assert!(!out.contains("LEAKC7") && !out.contains("hunter2"), "{out}");
    let back = crate::config::tree::parse(&c.new_docs.main().text)
        .unwrap()
        .to_json();
    assert_eq!(back["clients"]["home"]["secret"], "hunter2,LEAKC7");
}

#[test]
fn a_weak_secret_leaves_every_structural_field_as_it_is() {
    let w = world();
    let text = w.text.replace(&w.home.encode(), "home-error-secret");
    let core = super::core_running(&text, &w.text);
    let s = session(&w, AdminLevel::Write);
    let t = edit_text(&core, &s);
    let ops = replace(&t, |x| x.replace("# the home box", "# the home server"));
    let plan = planned(&core, &s, &ops).unwrap();
    assert!(!plan.valid);
    let weak = plan
        .diags
        .iter()
        .find(|d| d.code == "weak-secret")
        .unwrap_or_else(|| panic!("{:#?}", plan.diags));
    assert!(weak.is_error(), "{weak:?}");
    assert_eq!(
        weak.path.as_deref(),
        Some(&p(&["clients", "home", "secret"])[..])
    );
    assert_eq!(weak.file.as_deref(), Some("server.yaml"));
    assert!(
        weak.help
            .as_deref()
            .is_some_and(|h| h.contains("gensecret")),
        "{weak:?}"
    );
    let sent = serde_json::to_string(&plan).unwrap();
    assert!(!sent.contains("home-error-secret"), "{sent}");
    assert!(
        sent.contains("\"  home:\"") && sent.contains("\"severity\":\"error\""),
        "{sent}"
    );
    assert!(
        plan.diags
            .iter()
            .all(|d| !d.code.contains("«") && d.path.iter().flatten().all(|k| !k.contains("«")))
    );
}
