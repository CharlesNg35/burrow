use std::collections::{BTreeMap, BTreeSet};

use burrow_admin_proto::msg::{ErrorCode, LockedField};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::value::get;
use serde_json::Value;

use crate::admin::core::{ApiError, Core};
use crate::config::patch;

fn locked_error(locked: Vec<String>) -> ApiError {
    ApiError::new(
        ErrorCode::LockedField,
        format!(
            "{} is locked: change it in server.yaml on the server, then restart",
            locked.join(".")
        ),
    )
    .with(serde_json::to_value(LockedField { path: locked }).unwrap_or_default())
}

pub(super) fn refuse_locked(core: &Core, ops_list: &[Op]) -> Result<(), ApiError> {
    match ops_list
        .iter()
        .filter_map(|o| o.path())
        .find_map(|p| core.schema.locked_at(p))
    {
        Some(locked) => Err(locked_error(locked)),
        None => Ok(()),
    }
}

fn expand(pattern: &[String], doc: &Value, at: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    let Some((first, rest)) = pattern.split_first() else {
        out.push(at.clone());
        return;
    };
    let keys: Vec<String> = match (first.as_str(), doc) {
        (burrow_admin_proto::schema::ANY, Value::Object(m)) => m.keys().cloned().collect(),
        (burrow_admin_proto::schema::ANY, Value::Array(a)) => {
            (0..a.len()).map(|i| i.to_string()).collect()
        }
        (burrow_admin_proto::schema::ANY, _) => Vec::new(),
        (key, _) => vec![key.to_string()],
    };
    for k in keys {
        let Some(next) = burrow_admin_proto::value::get(doc, std::slice::from_ref(&k)) else {
            continue;
        };
        at.push(k);
        expand(rest, next, at, out);
        at.pop();
    }
}

pub(super) fn concrete_locks(
    schema: &burrow_admin_proto::schema::Schema,
    docs: [&Value; 2],
) -> Vec<(Vec<String>, bool, Vec<usize>)> {
    let mut out = Vec::new();
    for (l, lists) in schema.locks() {
        if !l.iter().any(|s| s == burrow_admin_proto::schema::ANY) {
            out.push((l, false, lists));
            continue;
        }
        let mut found = Vec::new();
        for doc in docs {
            expand(&l, doc, &mut Vec::new(), &mut found);
        }
        out.extend(found.into_iter().map(|p| (p, true, lists.clone())));
    }
    out.sort();
    out.dedup();
    out
}

pub(super) fn renamed_from(path: &[String], renames: &[patch::Rename]) -> Vec<String> {
    for r in renames {
        let Some((_, parent)) = r.path.split_last() else {
            continue;
        };
        if path.len() > parent.len() && path.starts_with(parent) && path[parent.len()] == r.to {
            let mut from = r.path.clone();
            from.extend_from_slice(&path[parent.len() + 1..]);
            return from;
        }
    }
    path.to_vec()
}

pub(super) fn refuse_changed_locks(
    schema: &burrow_admin_proto::schema::Schema,
    (before, after): (&Value, &Value),
    renames: &[patch::Rename],
) -> Result<(), ApiError> {
    let mut lists: BTreeMap<Vec<String>, BTreeSet<Vec<String>>> = BTreeMap::new();
    for (path, entry, at) in concrete_locks(schema, [before, after]) {
        let list = |k: usize| {
            let was = get(before, &renamed_from(&path[..k], renames));
            let then = get(after, &path[..k]);
            [was, then].iter().all(|v| v.is_none_or(Value::is_array))
                && [was, then].iter().any(|v| v.is_some())
        };
        if let Some(k) = at.into_iter().find(|&k| list(k)) {
            lists
                .entry(path[..k].to_vec())
                .or_default()
                .insert(path[k + 1..].to_vec());
            continue;
        }
        let then = get(after, &path);
        let was = get(before, &renamed_from(&path, renames));
        if then != was && (then.is_some() || !entry) {
            return Err(locked_error(path));
        }
    }
    for (list, fields) in lists {
        let fields: Vec<Vec<String>> = fields.into_iter().collect();
        let was = tuples(get(before, &renamed_from(&list, renames)), &fields);
        let then = tuples(get(after, &list), &fields);
        let added = then.iter().find(|(_, t)| {
            let count =
                |of: &[(usize, Vec<Option<&Value>>)]| of.iter().filter(|(_, x)| x == t).count();
            count(&then) > count(&was)
        });
        if let Some((i, t)) = added {
            let mut path = list.clone();
            path.push(i.to_string());
            let field = fields
                .iter()
                .zip(t)
                .find(|(_, v)| v.is_some())
                .map(|(f, _)| f);
            path.extend(field.into_iter().flatten().cloned());
            return Err(locked_error(path));
        }
    }
    Ok(())
}

fn tuples<'a>(
    list: Option<&'a Value>,
    fields: &[Vec<String>],
) -> Vec<(usize, Vec<Option<&'a Value>>)> {
    let Some(Value::Array(entries)) = list else {
        return Vec::new();
    };
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| (i, fields.iter().map(|f| get(e, f)).collect::<Vec<_>>()))
        .filter(|(_, t)| t.iter().any(Option::is_some))
        .collect()
}

fn shown(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "not set".into(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

pub(super) fn keep_locked(
    schema: &burrow_admin_proto::schema::Schema,
    current: &Value,
    target: &Value,
) -> (Value, Vec<burrow_admin_proto::plan::Impact>) {
    let mut out = target.clone();
    let mut notes = Vec::new();
    let kept = concrete_locks(schema, [current, target])
        .into_iter()
        .filter(|(p, entry, _)| {
            !entry
                || burrow_admin_proto::value::get(target, &p[..p.len() - 1])
                    .is_some_and(Value::is_object)
        })
        .map(|(p, _, _)| p);
    for path in kept {
        let now = burrow_admin_proto::value::get(current, &path).cloned();
        let then = burrow_admin_proto::value::get(target, &path).cloned();
        if now == then {
            continue;
        }
        if !burrow_admin_proto::value::put(&mut out, &path, now.clone()) {
            continue;
        }
        notes.push(burrow_admin_proto::plan::Impact {
            level: burrow_admin_proto::plan::Level::Info,
            code: "locked-kept".into(),
            message: format!(
                "{} kept at {} (locked)",
                path.join("."),
                shown(now.as_ref())
            ),
            confirm: None,
        });
    }
    (out, notes)
}
