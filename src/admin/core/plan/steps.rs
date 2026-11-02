use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::msg::ErrorCode;
use burrow_admin_proto::msg::WhyNot;
use burrow_admin_proto::plan::{Conflict, Impact, Level, Plan, Reformat};
use burrow_admin_proto::schema::Applies;
use burrow_config::ServerConfig;
use serde_json::Value;

use crate::admin::core::ops;
use crate::admin::core::{ApiError, Core, Session, guard, summary, text, values};
use crate::config::docs::{Docs, drop_in_folder_name};
use crate::config::patch;
use crate::config::settings::ServerSettings;

use super::locks::keep_locked;
use super::{Checked, applies, files, impact_key};

pub(super) fn conflict(core: &Core, base: &str, old_docs: &Docs, current: String) -> ApiError {
    let before = core.store.version(base).or_else(|| core.seen_docs(base));
    let theirs = before.as_ref().and_then(|b| {
        let (before, after) =
            values::hide_headers(&b.tree().ok()?.to_json(), &old_docs.tree().ok()?.to_json());
        Some(summary::changes(&before, &after, &[]))
    });
    let moved: Vec<String> = before
        .map(|b| crate::config::save::changes(&b, old_docs))
        .unwrap_or_default()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let message = if moved.iter().all(|n| *n == old_docs.main().name) {
        "the config changed on the server since you opened it; review your change again".to_string()
    } else {
        format!(
            "{} changed on disk since you opened it; review your change again",
            moved.join(", ")
        )
    };
    let body = Conflict {
        current: current.clone(),
        known: theirs.is_some(),
        theirs: theirs.unwrap_or_default(),
    };
    ApiError::new(ErrorCode::Conflict, message).with(serde_json::to_value(body).unwrap_or_default())
}

pub(super) fn parse_current(docs: &Docs) -> Result<Value, ApiError> {
    docs.tree()
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

fn failed(e: files::WriteError) -> ApiError {
    match e {
        files::WriteError::Patch(p) => ApiError::new(ErrorCode::Internal, p.to_string()),
        files::WriteError::Bad(m) => ApiError::new(ErrorCode::BadRequest, m),
    }
}

fn hold_read_only(core: &Core, current: &Docs, mut restored: Docs) -> (Docs, Vec<Impact>) {
    let mut notes = Vec::new();
    let names: Vec<String> = current
        .names()
        .chain(restored.names())
        .map(str::to_string)
        .collect();
    let mut seen = Vec::new();
    for name in names {
        if seen.contains(&name) || current.text_of(&name) == restored.text_of(&name) {
            continue;
        }
        seen.push(name.clone());
        let Some(why) = core.store.read_only(&name) else {
            continue;
        };
        restored.list.retain(|d| d.name != name);
        if let Some(d) = current.get(&name) {
            restored.list.push(d.clone());
        }
        notes.push(Impact {
            level: Level::Warning,
            code: burrow_admin_proto::codes::FILE_READ_ONLY.into(),
            message: format!(
                "{name} is read-only on disk ({why}), so it stays as it is; restore it with the tool that manages it"
            ),
            confirm: None,
        });
    }
    restored.sort();
    (restored, notes)
}

pub(super) fn new_document(
    core: &Core,
    raw: &Value,
    old_docs: &Docs,
    applied: &ops::Applied,
) -> Result<(Docs, Option<patch::Reformat>, Vec<Impact>), ApiError> {
    let format = core.store.format();
    let prefix = drop_in_folder_name(core.store.path());
    let moves = files::Moves {
        places: &applied.places,
        drops: &applied.drops,
        prefix: &prefix,
    };
    let mut kept = Vec::new();
    let (new_docs, reformat) = match &applied.document {
        Some(doc) => {
            let (target, notes) = keep_locked(&core.schema, raw, &applied.target);
            kept = notes;
            let docs = if kept.is_empty() {
                doc.clone()
            } else {
                files::write(doc, format, (&target, &[]), &files::Moves::NONE)
                    .map_err(failed)?
                    .docs
            };
            let (docs, held) = hold_read_only(core, old_docs, docs);
            kept.extend(held);
            (docs, None)
        }
        None => {
            let w = files::write(
                old_docs,
                format,
                (&applied.target, &applied.renames),
                &moves,
            )
            .map_err(failed)?;
            (w.docs, w.reformat)
        }
    };
    Ok((new_docs, reformat, kept))
}

pub(super) fn refuse_files(
    core: &Core,
    s: &Session,
    old: &Docs,
    new: &Docs,
) -> Result<(), ApiError> {
    let main = &old.main().name;
    for (name, _) in crate::config::save::changes(old, new) {
        let why = if let Some(why) = core.store.read_only(&name) {
            WhyNot {
                code: burrow_admin_proto::codes::FILE_READ_ONLY.into(),
                message: format!("read-only on disk: {name} ({why})"),
                hint: "change it with the tool or editor that manages it".into(),
            }
        } else if name != *main && !s.understands(burrow_admin_proto::features::CONFIG_FILES) {
            WhyNot {
                code: burrow_admin_proto::codes::OLDER_ADMIN_FILES.into(),
                message: format!("this admin is older than the server and cannot change {name}"),
                hint:
                    "use burrow admin from the same version as the server, or edit the file by hand"
                        .into(),
            }
        } else {
            continue;
        };
        return Err(ApiError::new(ErrorCode::NotWritable, why.message.clone())
            .with(serde_json::to_value(&why).unwrap_or_default()));
    }
    Ok(())
}

pub(super) struct Draft<'a> {
    pub(super) raw: &'a Value,
    pub(super) new_raw: &'a Value,
    pub(super) old_docs: &'a Docs,
    pub(super) new_docs: &'a Docs,
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
    let shared = guard::shared_web_port(running, (raw, new_raw));
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
        clash.extend(guard::busy_web_ports(
            &|a| core.live.bind_probe(a),
            running,
            next,
        ));
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
    refusals.extend(shared);
    Verdict {
        refusals,
        impacts,
        clash,
    }
}

fn listen_clash(c: crate::config::settings::ListenClash) -> Diag {
    Diag {
        severity: burrow_admin_proto::diag::Severity::Error,
        code: burrow_admin_proto::codes::LISTEN_CLASH.into(),
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
        subjects: Vec::new(),
        file: None,
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
    let (old_docs, new_docs, raw, new_raw, applied) =
        (d.old_docs, d.new_docs, d.raw, d.new_raw, d.applied);
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
    let root = &core.schema().root;
    let (mut hashes, mut headers) = (Vec::new(), Vec::new());
    for v in [raw, new_raw] {
        values::hashes(v, Some(root), &mut hashes);
        values::header_values(v, &mut headers);
    }
    let mask = |l: &str| text::mask_secrets(&values::veil_line(l, &hashes, &headers), &fresh_pairs);
    let diff =
        crate::admin::core::diff::unified(&old_docs.main().text, &new_docs.main().text, &mask);
    let file_diffs = files::diffs(old_docs, new_docs, &mask);
    let (old_shown, new_shown) = values::hide_headers(raw, new_raw);
    let mut changes = summary::changes(
        &values::veil_value(&old_shown, &hashes),
        &values::veil_value(&new_shown, &hashes),
        &applied.renames,
    );
    files::note_moves(old_docs, applied, &mut changes);
    let impacts_json =
        serde_json::to_vec(&impacts.iter().map(impact_key).collect::<Vec<_>>()).unwrap_or_default();
    let refusals_json = serde_json::to_vec(&refusals).unwrap_or_default();
    let hash = text::hash_parts(&[
        base.as_bytes(),
        new_docs.bundle().as_bytes(),
        &impacts_json,
        &refusals_json,
    ]);
    Plan {
        plan: hash,
        base: base.to_string(),
        result_rev: text::docs_rev(new_docs),
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
        files: file_diffs,
    }
}
