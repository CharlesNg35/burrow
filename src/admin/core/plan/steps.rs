use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::msg::ErrorCode;
use burrow_admin_proto::plan::{Conflict, Plan, Reformat};
use burrow_admin_proto::schema::Applies;
use burrow_config::ServerConfig;
use serde_json::Value;

use crate::admin::core::ops;
use crate::admin::core::{ApiError, Core, Session, guard, summary, text};
use crate::config::settings::ServerSettings;
use crate::config::{patch, tree};

use super::locks::keep_locked;
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
    let root = core.schema().root.clone();
    let mut hashes = Vec::new();
    for v in [raw, new_raw] {
        crate::admin::core::values::hashes(v, Some(&root), &mut hashes);
    }
    let mask =
        |l: &str| text::mask_secrets(&crate::admin::core::values::veil(l, &hashes), &fresh_pairs);
    let diff = crate::admin::core::diff::unified(old_text, new_text, &mask);
    let changes = summary::changes(
        &crate::admin::core::values::veil_value(raw, &hashes),
        &crate::admin::core::values::veil_value(new_raw, &hashes),
        &applied.renames,
    );
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
