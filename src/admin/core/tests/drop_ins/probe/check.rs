#![cfg(test)]

use std::collections::BTreeMap;

use burrow_admin_proto::msg::{Apply, ErrorCode};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{Applied, Plan};
use serde_json::Value;

use super::model::{Model, locations, simulate};
use super::{Item, MAIN, SECTIONS, Set, disk, file_items};
use crate::admin::core::plan::{self, ReloadResult};
use crate::admin::core::{ApiError, Session, ops};

type Files = BTreeMap<String, String>;

fn vdiff(a: &Value, b: &Value, at: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                let (p, q) = (
                    x.get(k).unwrap_or(&Value::Null),
                    y.get(k).unwrap_or(&Value::Null),
                );
                if p != q {
                    vdiff(p, q, &format!("{at}.{k}"), out);
                }
            }
        }
        _ if a != b => out.push(format!("{at}: {a} vs {b}")),
        _ => {}
    }
}

fn show_diff(a: &Value, b: &Value) -> String {
    let mut out = Vec::new();
    vdiff(a, b, "", &mut out);
    out.dedup();
    out.join("\n")
}

fn norm(v: &Value) -> Value {
    let mut v = v.clone();
    if let Value::Object(top) = &mut v {
        top.retain(|k, x| {
            !(SECTIONS.contains(&k.as_str())
                && (x.is_null() || x.as_object().is_some_and(|m| m.is_empty())))
        });
    }
    v
}

pub(super) fn merged_of(files: &BTreeMap<String, String>) -> Value {
    let docs = crate::config::docs::Docs {
        list: files
            .iter()
            .filter(|(n, _)| *n == MAIN)
            .chain(files.iter().filter(|(n, _)| *n != MAIN))
            .map(|(n, t)| crate::config::docs::Doc {
                name: n.clone(),
                text: t.clone(),
            })
            .collect(),
    };
    let mut d = docs;
    d.sort();
    norm(&d.tree().map(|t| t.to_json()).unwrap_or(Value::Null))
}

#[derive(Default, Debug)]
pub(super) struct Stats {
    applied: usize,
    refused: usize,
    bad_request: BTreeMap<String, usize>,
    ambiguous: usize,
}

impl Stats {
    pub(super) fn applied(&self) -> usize {
        self.applied
    }

    pub(super) fn said(&self) -> String {
        format!(
            "applied {}, refused {}, ambiguous {}, bad requests {:?}",
            self.applied, self.refused, self.ambiguous, self.bad_request
        )
    }
}

fn short(m: &str) -> String {
    m.split(['(', '"'])
        .next()
        .unwrap_or(m)
        .chars()
        .filter(|c| !c.is_ascii_digit())
        .take(70)
        .collect()
}

const OK_BAD: [&str; 5] = [
    "cannot be deleted: it still holds items",
    "already exists",
    "does not exist",
    "is not one of the config files",
    "differ only in upper and lower case",
];

fn plan_failed(
    set: &Set,
    before: &Files,
    ops: &[Op],
    e: &ApiError,
    stats: &mut Stats,
) -> Result<(), String> {
    if disk(set) != *before {
        return Err("e-write: a failed plan changed files".into());
    }
    if e.code == ErrorCode::Internal {
        return Err(format!("e-internal: {}", e.message));
    }
    if e.message.contains("cannot be deleted") {
        let m = simulate(before, ops);
        let predicted_empty = m
            .drops
            .iter()
            .all(|f| !m.items.values().any(|t| t.expected() == *f));
        let maybe_kept = m
            .items
            .values()
            .any(|t| t.placed.is_none() && t.orig.is_none() && t.also_ok.is_some());
        if predicted_empty && !maybe_kept {
            return Err(format!(
                "e-drop: a file the model sees empty was refused: {}",
                e.message
            ));
        }
    }
    if e.code == ErrorCode::BadRequest && OK_BAD.iter().any(|m| e.message.contains(m)) {
        *stats.bad_request.entry(short(&e.message)).or_default() += 1;
        return Ok(());
    }
    Err(format!("e-error: {:?} {}", e.code, e.message))
}

fn no_secret_shown(planned: &Plan, secrets: &[String]) -> Result<(), String> {
    let shown = serde_json::to_string(planned).unwrap();
    for sec in secrets {
        let core = sec.strip_prefix("Basic ").unwrap_or(sec);
        if shown.contains(core) {
            return Err(format!("d-secret: {core} shown in plan"));
        }
    }
    Ok(())
}

pub(super) fn run_change(
    set: &Set,
    s: &Session,
    secrets: &[String],
    ops: &[Op],
    stats: &mut Stats,
) -> Result<(), String> {
    let before = disk(set);
    let base = set.base();
    let computed = match plan::compute(&set.core, s, &base, ops) {
        Ok(c) => c,
        Err(e) => return plan_failed(set, &before, ops, &e, stats),
    };
    let planned = &computed.plan;
    no_secret_shown(planned, secrets)?;
    let raw = set.core.docs().unwrap().tree().unwrap().to_json();
    let applied = ops::apply(&raw, ops, &NoCx).map_err(|e| format!("e-ops: {}", e.0))?;
    let intended = norm(&applied.target);
    let new_tree = computed.new_docs.tree().map(|t| t.to_json());
    let new_merged = norm(&new_tree.unwrap_or(Value::Null));
    if intended != new_merged {
        return Err(format!(
            "a-plan: plan's new docs merge differs from the ops target\n{}",
            show_diff(&intended, &new_merged)
        ));
    }
    if !planned.valid || !planned.refusals.is_empty() {
        if apply_with(set, s, &base, ops, planned).is_ok() {
            return Err("e-refused: an invalid plan applied".into());
        }
        if disk(set) != before {
            return Err("e-write: a refused plan wrote files".into());
        }
        stats.refused += 1;
        return Ok(());
    }
    if let Err(e) = apply_with(set, s, &base, ops, planned) {
        return Err(format!("e-apply: {:?} {}", e.code, e.message));
    }
    stats.applied += 1;
    let after = disk(set);
    let planned_files: Files = computed
        .new_docs
        .list
        .iter()
        .map(|d| (d.name.clone(), d.text.clone()))
        .collect();
    if after != planned_files {
        let diff: Vec<&String> = after
            .keys()
            .chain(planned_files.keys())
            .filter(|k| after.get(*k) != planned_files.get(*k))
            .collect();
        return Err(format!(
            "a-disk: disk differs from the plan's files: {diff:?}"
        ));
    }
    let disk_merged = merged_of(&after);
    if disk_merged != intended {
        return Err(format!(
            "a-merged: merged disk differs from intended\n{}",
            show_diff(&intended, &disk_merged)
        ));
    }
    let model = simulate(&before, ops);
    items_in_place(&model, (&before, &after), stats)?;
    files_kept(&before, &after)?;
    comments_kept(&model, (&before, &after), planned)
}

fn items_in_place(
    model: &Model,
    (before, after): (&Files, &Files),
    stats: &mut Stats,
) -> Result<(), String> {
    let locs = locations(after);
    if let Some((it, at)) = locs.iter().find(|(_, at)| at.len() > 1) {
        return Err(format!("b-dup: {}.{} in {at:?}", it.0, it.1));
    }
    for (it, t) in &model.items {
        let Some(at) = locs.get(it) else {
            return Err(format!("b-lost: {}.{} missing on disk", it.0, it.1));
        };
        let exp = t.expected();
        if at[0] == exp {
            continue;
        }
        if t.placed.is_none() && t.also_ok.as_deref() == Some(at[0].as_str()) {
            stats.ambiguous += 1;
            continue;
        }
        let k = if t.orig.is_none() && t.placed.is_none() {
            "b-fresh"
        } else {
            "b-file"
        };
        return Err(format!(
            "{k}: {}.{} is in {} but expected {exp} (track {t:?})",
            it.0, it.1, at[0]
        ));
    }
    if let Some(it) = locs.keys().find(|it| !model.items.contains_key(*it)) {
        return Err(format!(
            "b-extra: {}.{} on disk but not expected",
            it.0, it.1
        ));
    }
    for f in before.keys() {
        let dropped = model.drops.contains(f);
        if dropped == after.contains_key(f) {
            return Err(format!(
                "b-files: {f} dropped={dropped} but exists={}",
                after.contains_key(f)
            ));
        }
    }
    Ok(())
}

fn files_kept(before: &Files, after: &Files) -> Result<(), String> {
    for (f, bt) in before {
        let Some(at) = after.get(f) else { continue };
        if file_items(bt) == file_items(at) && bt != at {
            return Err(format!(
                "c-bytes: {f} rewritten though its items did not change\n--- before\n{bt}--- after\n{at}"
            ));
        }
        if let Some(h) = bt.lines().find(|l| l.starts_with("# header"))
            && !at.contains(h)
        {
            return Err(format!("c-header: {f} lost its header comment\n{at}"));
        }
    }
    Ok(())
}

fn tag_of(text: &str, name: &str) -> Option<String> {
    text.lines()
        .find(|l| {
            l.trim_start().starts_with(&format!("{name}:"))
                && l.starts_with("  ")
                && !l.starts_with("   ")
        })
        .and_then(|l| l.find("# id").map(|i| l[i..].to_string()))
}

fn comments_kept(
    model: &Model,
    (before, after): (&Files, &Files),
    planned: &Plan,
) -> Result<(), String> {
    let before_items: BTreeMap<Item, Value> = before
        .values()
        .flat_map(|t| file_items(t).unwrap_or_default())
        .collect();
    let after_items: BTreeMap<Item, Value> = after
        .values()
        .flat_map(|t| file_items(t).unwrap_or_default())
        .collect();
    for (it, t) in &model.items {
        let (Some(of), Some(on)) = (&t.orig, &t.orig_name) else {
            continue;
        };
        let was = before_items.get(&(it.0.clone(), on.clone()));
        if t.expected() != *of || was.is_none() || after_items.get(it) != was {
            continue;
        }
        let Some(tag) = tag_of(&before[of], on) else {
            continue;
        };
        let now = &after[of];
        if !now.contains(&format!("{tag}\n")) && !now.contains(&format!("{tag} ")) {
            let k = if planned.reformat.is_some() {
                "c-reformat"
            } else {
                "c-comment"
            };
            return Err(format!(
                "{k}: reformat={:?} untouched {}.{} lost comment '{tag}' in {of}\n--- before\n{}--- after\n{now}",
                planned.reformat, it.0, it.1, before[of]
            ));
        }
    }
    Ok(())
}

struct NoCx;

impl ops::Context for NoCx {
    fn secret(&self, _: &str) -> Option<(String, String)> {
        None
    }
    fn password(&self, _: &str) -> Option<String> {
        None
    }
    fn history(&self, _: &str) -> Option<crate::config::docs::Docs> {
        None
    }
    fn env(&self) -> &dyn crate::config::env::EnvSource {
        &NOENV
    }
}

static NOENV: fn(&str) -> Option<String> = |_| None;

fn apply_with(
    set: &Set,
    s: &Session,
    base: &str,
    ops: &[Op],
    planned: &Plan,
) -> Result<Applied, ApiError> {
    let begun = plan::apply_begin(
        &set.core,
        s,
        &Apply {
            base: base.to_string(),
            ops: ops.to_vec(),
            confirm: planned.confirms().iter().map(|c| c.to_string()).collect(),
            plan: planned.plan.clone(),
            address: None,
        },
    )?;
    plan::finish(&set.core, s, begun, ReloadResult::unchanged(), None)
}
