use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::msg::{ErrorCode, LockedField};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{Conflict, Plan, Reformat};
use burrow_admin_proto::schema::Applies;
use burrow_config::ServerConfig;
use serde_json::Value;

use crate::admin::core::ops;
use crate::admin::core::{ApiError, Core, Session, guard, summary, text};
use crate::config::settings::ServerSettings;
use crate::config::{patch, tree};

use super::{Checked, applies, impact_key};

pub(super) fn conflict(core: &Core, base: &str, old_text: &str, current: String) -> ApiError {
    let theirs = core
        .store
        .version(base)
        .or_else(|| core.seen_text(base))
        .and_then(|b| {
            let (_, _, a) = patch::split_text(&b);
            let (_, _, c) = patch::split_text(old_text);
            Some(summary::changes(
                &tree::parse(&a).ok()?.to_json(),
                &tree::parse(&c).ok()?.to_json(),
                &[],
            ))
        });
    let body = Conflict {
        current: current.clone(),
        known: theirs.is_some(),
        theirs: theirs.unwrap_or_default(),
    };
    ApiError::new(
        ErrorCode::Conflict,
        "the config changed on the server since you opened it; review your change again",
    )
    .with(serde_json::to_value(body).unwrap_or_default())
}

pub(super) fn parse_current(src: &str) -> Result<Value, ApiError> {
    tree::parse(src)
        .map_err(|e| {
            ApiError::new(
                ErrorCode::Invalid,
                format!(
                    "the config file on the server does not parse ({}); fix it by hand first",
                    e.first().map(|e| e.message.clone()).unwrap_or_default()
                ),
            )
        })
        .map(|t| t.to_json())
}

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

fn concrete_locks(
    schema: &burrow_admin_proto::schema::Schema,
    docs: [&Value; 2],
) -> Vec<(Vec<String>, bool)> {
    let mut out = Vec::new();
    for l in schema.locked_paths() {
        if !l.iter().any(|s| s == burrow_admin_proto::schema::ANY) {
            out.push((l, false));
            continue;
        }
        let mut found = Vec::new();
        for doc in docs {
            expand(&l, doc, &mut Vec::new(), &mut found);
        }
        out.extend(found.into_iter().map(|p| (p, true)));
    }
    out.sort();
    out.dedup();
    out
}

fn renamed_from(path: &[String], renames: &[patch::Rename]) -> Vec<String> {
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
    for (path, entry) in concrete_locks(schema, [before, after]) {
        let then = burrow_admin_proto::value::get(after, &path);
        let was = burrow_admin_proto::value::get(before, &renamed_from(&path, renames));
        if then != was && (then.is_some() || !entry) {
            return Err(locked_error(path));
        }
    }
    Ok(())
}

pub(super) fn new_document(
    core: &Core,
    raw: &Value,
    old_text: &str,
    applied: &ops::Applied,
) -> Result<
    (
        String,
        Option<patch::Reformat>,
        Vec<burrow_admin_proto::plan::Impact>,
    ),
    ApiError,
> {
    let mut kept = Vec::new();
    let (new_text, reformat) = match &applied.document {
        Some(doc) => {
            let (target, notes) = keep_locked(&core.schema, raw, &applied.target);
            kept = notes;
            if kept.is_empty() {
                (doc.clone(), None)
            } else {
                let p = patch::patch(doc, core.store.format(), &target, &[])
                    .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
                (p.text, None)
            }
        }
        None => {
            let p = patch::patch(
                old_text,
                core.store.format(),
                &applied.target,
                &applied.renames,
            )
            .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
            (p.text, p.reformat)
        }
    };
    Ok((new_text, reformat, kept))
}

pub(super) struct Draft<'a> {
    pub(super) raw: &'a Value,
    pub(super) new_raw: &'a Value,
    pub(super) old_text: &'a str,
    pub(super) new_text: &'a str,
    pub(super) applied: &'a ops::Applied,
}

pub(super) struct Verdict {
    pub(super) refusals: Vec<burrow_admin_proto::plan::Refusal>,
    pub(super) impacts: Vec<burrow_admin_proto::plan::Impact>,
    pub(super) clash: Vec<Diag>,
}

pub(super) fn judge(
    core: &Core,
    s: &Session,
    d: &Draft,
    running: &ServerSettings,
    file: &Checked,
    checked: &Checked,
) -> Verdict {
    let (raw, new_raw, applied) = (d.raw, d.new_raw, d.applied);
    let (mut refusals, mut impacts, mut clash) = (Vec::new(), Vec::new(), Vec::new());
    if let Some(next) = &checked.settings {
        let who = guard::Who {
            visitor: &s.visitor,
            peer: s.peer,
            secret: &s.secret,
            via_websocket: s.via_websocket,
        };
        let base = match (&file.settings, file.valid) {
            (Some(settings), true) => guard::Base::Valid(settings),
            _ => guard::Base::Broken {
                raw,
                next_raw: new_raw,
            },
        };
        refusals = guard::refusals(
            &who,
            &guard::Change {
                running,
                base,
                next,
                renames: &applied.renames,
                path: core.store.path(),
            },
        );
        if let Some(c) = running.listen_clash(next) {
            clash.push(listen_clash(c));
        }
        let how = applies(core, raw, &applied.target);
        let restart: Vec<String> = how
            .iter()
            .filter(|a| a.how == Applies::Restart)
            .map(|a| a.field.clone())
            .collect();
        impacts = guard::impacts(
            running,
            next,
            &core.live.status(),
            &core.live.sessions(),
            &restart,
            &applied.renames,
        );
    }
    Verdict {
        refusals,
        impacts,
        clash,
    }
}

fn listen_clash(c: crate::config::settings::ListenClash) -> Diag {
    Diag {
        severity: burrow_admin_proto::diag::Severity::Error,
        code: "listen-clash".into(),
        message: format!(
            "{} uses {}, which this server keeps listening on until it restarts",
            c.what, c.kept
        ),
        path: Some(c.path),
        also: vec![ServerConfig::LISTEN.path()],
        label: None,
        help: Some("restart the server after changing listen, then use the old address".into()),
        line: None,
        col: None,
        before: false,
    }
}

pub(super) fn assemble(
    core: &Core,
    base: &str,
    d: &Draft,
    checked: Checked,
    file: Checked,
    v: Verdict,
    reformat: Option<patch::Reformat>,
) -> Plan {
    let (old_text, new_text, raw, new_raw, applied) =
        (d.old_text, d.new_text, d.raw, d.new_raw, d.applied);
    let Verdict {
        refusals,
        impacts,
        clash,
    } = v;
    let fresh_pairs: Vec<(String, String)> = applied
        .fresh
        .iter()
        .map(|f| (f.value.clone(), f.fp.clone()))
        .collect();
    let mask = |l: &str| text::mask_secrets(l, &fresh_pairs);
    let diff = crate::admin::core::diff::unified(old_text, new_text, &mask);
    let changes = summary::changes(raw, new_raw, &applied.renames);
    let impacts_json =
        serde_json::to_vec(&impacts.iter().map(impact_key).collect::<Vec<_>>()).unwrap_or_default();
    let refusals_json = serde_json::to_vec(&refusals).unwrap_or_default();
    let hash = text::hash_parts(&[
        base.as_bytes(),
        new_text.as_bytes(),
        &impacts_json,
        &refusals_json,
    ]);
    Plan {
        plan: hash,
        base: base.to_string(),
        result_rev: text::rev(new_text),
        valid: checked.valid && clash.is_empty(),
        diags: checked
            .diags
            .into_iter()
            .chain(clash)
            .map(|mut d| {
                d.before = file.diags.iter().any(|o| {
                    o.severity == d.severity
                        && o.code == d.code
                        && o.path == d.path
                        && o.message == d.message
                });
                d
            })
            .collect(),
        changes,
        diff,
        impacts,
        refusals,
        applies: applies(core, raw, new_raw),
        reformat: reformat.map(|r| Reformat {
            scope: r.scope,
            reason: r.reason,
        }),
        secrets_in_diff: "masked".into(),
    }
}

fn shown(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "not set".into(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

fn keep_locked(
    schema: &burrow_admin_proto::schema::Schema,
    current: &Value,
    target: &Value,
) -> (Value, Vec<burrow_admin_proto::plan::Impact>) {
    let mut out = target.clone();
    let mut notes = Vec::new();
    let kept = concrete_locks(schema, [current, target])
        .into_iter()
        .filter(|(p, entry)| {
            !entry
                || burrow_admin_proto::value::get(target, &p[..p.len() - 1])
                    .is_some_and(Value::is_object)
        })
        .map(|(p, _)| p);
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
