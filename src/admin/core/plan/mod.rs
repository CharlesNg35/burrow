use std::collections::BTreeSet;

use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::msg::{AuditEntry, ErrorCode, LockedField};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{
    Applied, ApplyHow, Conflict, Plan, Reformat, ReloadFailed, RevealedSecret,
};
use burrow_admin_proto::schema::{Applies, SemType};
use burrow_config::ServerConfig;
use serde_json::Value;

use super::diags::Mapper;
use super::ops::{self, Fresh};
use super::ports::WriteError;
use super::{ApiError, Core, Session, guard, summary, text};
use crate::config::settings::ServerSettings;
use crate::config::{Loaded, Role, load::load_str_in, patch, tree};

pub struct Checked {
    pub raw: Option<Value>,
    pub diags: Vec<Diag>,
    pub valid: bool,
    pub settings: Option<ServerSettings>,
}

pub fn check_text(core: &Core, text: &str) -> Checked {
    let src = text.strip_prefix('\u{feff}').unwrap_or(text);
    let dir = core
        .store
        .path()
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let outcome = load_str_in(
        &dir,
        &core.store.path().display().to_string(),
        text,
        core.store.format(),
        Some(Role::Server),
        &*core.env,
    );
    let parsed = tree::parse(src).ok();
    let mapper = Mapper::new(parsed.as_ref());
    let diags: Vec<Diag> = outcome
        .report
        .diagnostics
        .iter()
        .map(|d| mapper.convert(d))
        .collect();
    let settings = match outcome.loaded {
        Some(Loaded::Server(s, _)) => Some(*s),
        _ => None,
    };
    Checked {
        raw: parsed.map(|p| p.to_json()),
        valid: !outcome.report.has_errors(),
        diags,
        settings,
    }
}

pub fn used_ports(core: &Core, text: &str) -> BTreeSet<u16> {
    let mut out = BTreeSet::new();
    if let Some(s) = check_text(core, text).settings {
        out.extend(s.listen.map(|a| a.port()));
        out.extend(s.websocket.as_ref().map(|w| w.listen.port()));
        out.extend(s.metrics.map(|a| a.port()));
        for svc in s.services.values() {
            out.extend(svc.binds().into_iter().map(|(_, a)| a.port()));
        }
    }
    out
}

pub struct Computed {
    pub plan: Plan,
    pub old_text: String,
    pub new_text: String,
    pub fresh: Vec<Fresh>,
    pub reverts: Option<String>,
}

struct Cx<'a> {
    core: &'a Core,
    session: &'a Session,
}

impl ops::Context for Cx<'_> {
    fn secret(&self, handle: &str) -> Option<(String, String)> {
        self.session.pending_secret(handle)
    }
    fn history(&self, rev: &str) -> Option<String> {
        self.core.store.version(rev)
    }
    fn env(&self) -> &dyn crate::config::env::EnvSource {
        &*self.core.env
    }
}

fn applies(core: &Core, old: &Value, new: &Value) -> Vec<ApplyHow> {
    let SemType::Object(top) = &core.schema().root.sem else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for f in &top.fields {
        let (a, b) = (old.get(&f.key), new.get(&f.key));
        if a == b {
            continue;
        }
        if let Some(how) = f.node.applies {
            out.push(ApplyHow {
                field: f.key.clone(),
                how,
            });
            continue;
        }
        let inner = match &f.node.sem {
            SemType::Optional(o) => &o.inner.sem,
            s => s,
        };
        if let SemType::Object(o) = inner {
            for c in &o.fields {
                let (x, y) = (a.and_then(|v| v.get(&c.key)), b.and_then(|v| v.get(&c.key)));
                if x != y {
                    out.push(ApplyHow {
                        field: format!("{}.{}", f.key, c.key),
                        how: c.node.applies.unwrap_or(Applies::Reload),
                    });
                }
            }
        }
    }
    out
}

fn impact_key(i: &burrow_admin_proto::plan::Impact) -> String {
    let mut message = String::with_capacity(i.message.len());
    let mut in_number = false;
    for c in i.message.chars() {
        if c.is_ascii_digit() {
            if !in_number {
                message.push('#');
            }
            in_number = true;
        } else {
            in_number = false;
            message.push(c);
        }
    }
    format!(
        "{:?}|{}|{:?}|{}",
        i.level,
        i.code,
        i.confirm,
        message.replace("connections", "connection")
    )
}

pub fn compute(
    core: &Core,
    s: &Session,
    base: &str,
    ops_list: &[Op],
) -> Result<Computed, ApiError> {
    let old_text = core.read()?;
    let current = text::rev(&old_text);
    if current != base {
        return Err(conflict(core, base, &old_text, current));
    }
    let (_, _, src) = patch::split_text(&old_text);
    let raw = parse_current(&src)?;
    refuse_locked(core, ops_list)?;
    let applied = ops::apply(&raw, ops_list, &Cx { core, session: s })
        .map_err(|e| ApiError::new(ErrorCode::BadRequest, e.0))?;
    let (new_text, reformat, kept) = new_document(core, &raw, &old_text, &applied)?;
    let checked = check_text(core, &new_text);
    let new_raw = tree::parse(&patch::split_text(&new_text).2)
        .map(|n| n.to_json())
        .unwrap_or(Value::Null);
    let running = core.live.settings();
    let file = check_text(core, &old_text);
    let d = Draft {
        raw: &raw,
        new_raw: &new_raw,
        old_text: &old_text,
        new_text: &new_text,
        applied: &applied,
    };
    let mut v = judge(core, s, &d, &running, &file, &checked);
    v.impacts.extend(kept);
    let plan = assemble(core, base, &d, checked, file, v, reformat);
    Ok(Computed {
        plan,
        old_text,
        new_text,
        fresh: applied.fresh,
        reverts: ops_list.iter().find_map(|o| match o {
            Op::ReplaceDocument { from_history } => Some(from_history.clone()),
            _ => None,
        }),
    })
}

fn conflict(core: &Core, base: &str, old_text: &str, current: String) -> ApiError {
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

fn parse_current(src: &str) -> Result<Value, ApiError> {
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

fn refuse_locked(core: &Core, ops_list: &[Op]) -> Result<(), ApiError> {
    if let Some(locked) = ops_list
        .iter()
        .filter_map(|o| o.path())
        .find_map(|p| core.schema.locked_at(p))
    {
        return Err(ApiError::new(
            ErrorCode::LockedField,
            format!(
                "{} is locked: change it in server.yaml on the server, then restart",
                locked.join(".")
            ),
        )
        .with(serde_json::to_value(LockedField { path: locked }).unwrap_or_default()));
    }
    Ok(())
}

fn new_document(
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

struct Draft<'a> {
    raw: &'a Value,
    new_raw: &'a Value,
    old_text: &'a str,
    new_text: &'a str,
    applied: &'a ops::Applied,
}

struct Verdict {
    refusals: Vec<burrow_admin_proto::plan::Refusal>,
    impacts: Vec<burrow_admin_proto::plan::Impact>,
    clash: Vec<Diag>,
}

fn judge(
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

fn assemble(
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
    let diff = super::diff::unified(old_text, new_text, &mask);
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
    for path in schema.locked_paths() {
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

pub struct Begun {
    pub computed: Computed,
    pub wrote: bool,
    pub address: Option<String>,
}

pub fn apply_begin(
    core: &Core,
    s: &Session,
    req: &burrow_admin_proto::msg::Apply,
) -> Result<Begun, ApiError> {
    if !s.writes() {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "read access cannot change the config",
        ));
    }
    if let Err(w) = core.store.writable() {
        return Err(
            ApiError::new(ErrorCode::NotWritable, w.message.clone()).with(serde_json::json!({
                "code": w.code, "hint": w.hint
            })),
        );
    }
    let computed = compute(core, s, &req.base, &req.ops)?;
    let plan = &computed.plan;
    if plan.plan != req.plan {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "the plan changed since you reviewed it (the live state or the file moved); review it again",
        )
        .with(serde_json::json!({ "current": plan.base, "plan_changed": true })));
    }
    if !plan.valid {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            format!("the change has {} errors", plan.errors()),
        )
        .with(serde_json::json!({ "diags": plan.diags })));
    }
    if !plan.refusals.is_empty() {
        return Err(ApiError::new(
            ErrorCode::LockedOut,
            plan.refusals
                .iter()
                .map(|r| r.message.clone())
                .collect::<Vec<_>>()
                .join("; "),
        )
        .with(serde_json::json!({ "reasons": plan.refusals })));
    }
    let missing: Vec<&str> = plan
        .confirms()
        .into_iter()
        .filter(|c| !req.confirm.iter().any(|g| g == c))
        .collect();
    if !missing.is_empty() {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            format!("confirm this change by typing: {}", missing.join(", ")),
        )
        .with(serde_json::json!({ "confirm": missing })));
    }
    if computed.new_text == computed.old_text {
        return Ok(Begun {
            computed,
            wrote: false,
            address: req.address.clone(),
        });
    }
    core.store.keep(&computed.plan.base, &computed.old_text);
    match core.store.write(&computed.old_text, &computed.new_text) {
        Ok(()) => {}
        Err(WriteError::Changed) => {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "the config changed on the server while applying; review your change again",
            )
            .with(serde_json::json!({ "current": "" })));
        }
        Err(WriteError::Io(e)) => {
            return Err(ApiError::new(
                ErrorCode::NotWritable,
                format!("cannot write the config: {e}"),
            ));
        }
    }
    core.store
        .keep(&computed.plan.result_rev, &computed.new_text);
    Ok(Begun {
        computed,
        wrote: true,
        address: req.address.clone(),
    })
}

pub enum ReloadResult {
    Applied {
        restart: Vec<String>,
        unbound: Vec<String>,
    },
    Rejected(String),
    Timeout,
}

pub fn restore(core: &Core, begun: &Begun) -> bool {
    begun.wrote
        && core
            .store
            .write(&begun.computed.new_text, &begun.computed.old_text)
            .is_ok()
}

pub fn finish(
    core: &Core,
    s: &Session,
    begun: Begun,
    reload: ReloadResult,
    restored: Option<bool>,
) -> Result<Applied, ApiError> {
    let plan = &begun.computed.plan;
    let entry = |result: &str| AuditEntry {
        at: text::now_unix(),
        visitor: s.visitor.clone(),
        from: s.peer.ip().to_string(),
        base: plan.base.clone(),
        rev: plan.result_rev.clone(),
        changes: plan.changes.clone(),
        result: result.into(),
        revertible: true,
        details_kept: true,
        reverts: begun.computed.reverts.clone(),
    };
    let summary: Vec<&str> = plan.changes.iter().map(|c| c.summary.as_str()).collect();
    match reload {
        ReloadResult::Rejected(why) => {
            let restored = restored.unwrap_or(false);
            tracing::warn!(
                target: "burrow::admin",
                visitor = %s.visitor,
                from = %s.peer.ip(),
                "the server rejected the config written by burrow admin ({why}); {}",
                if restored { "the previous file was written back" } else { "the previous file could NOT be written back" }
            );
            core.record(entry(if restored {
                "rejected, restored"
            } else {
                "rejected"
            }));
            let body = ReloadFailed {
                restored,
                reason: why.clone(),
                hint: if restored {
                    String::new()
                } else {
                    format!("fix {} on the server by hand", core.store.path().display())
                },
            };
            Err(ApiError::new(
                ErrorCode::ReloadFailed,
                format!("the server did not take the new config: {why}"),
            )
            .with(serde_json::to_value(body).unwrap_or_default()))
        }
        ReloadResult::Timeout | ReloadResult::Applied { .. } => {
            let (restart, unbound) = match &reload {
                ReloadResult::Applied { restart, unbound } => (restart.clone(), unbound.clone()),
                _ => (Vec::new(), Vec::new()),
            };
            tracing::info!(
                target: "burrow::admin",
                visitor = %s.visitor,
                from = %s.peer.ip(),
                base = %plan.base,
                rev = %plan.result_rev,
                changes = %summary.join("; "),
                "config changed by admin"
            );
            core.record(entry(if unbound.is_empty() {
                "applied"
            } else {
                "applied, some ports could not listen"
            }));
            let handles: Vec<String> = begun
                .computed
                .fresh
                .iter()
                .map(|f| f.handle.clone())
                .collect();
            let secrets = begun
                .computed
                .fresh
                .iter()
                .map(|f| RevealedSecret {
                    handle: f.handle.clone(),
                    path: f.path.clone(),
                    fp: f.fp.clone(),
                    value: f.value.clone(),
                    shown_once: true,
                })
                .collect();
            s.consume(&handles);
            let snippets =
                super::snippets::after_apply(core, &begun.computed, begun.address.as_deref());
            Ok(Applied {
                rev: plan.result_rev.clone(),
                applied: plan.applies.clone(),
                restart_needed: restart,
                secrets,
                snippets,
            })
        }
    }
}
