use burrow_admin_proto::msg::{AuditEntry, ErrorCode};
use burrow_admin_proto::plan::{Applied, ReloadFailed, RevealedSecret};

use crate::admin::core::ports::WriteError;
use crate::admin::core::{ApiError, Core, Session, text};

use super::{Computed, compute};

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
        lines: Vec<String>,
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
            let (restart, unbound, not_listening) = match &reload {
                ReloadResult::Applied {
                    restart,
                    unbound,
                    lines,
                } => (restart.clone(), unbound.clone(), lines.clone()),
                _ => (Vec::new(), Vec::new(), Vec::new()),
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
                .chain(begun.computed.passwords.iter().cloned())
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
            let snippets = crate::admin::core::snippets::after_apply(
                core,
                &begun.computed,
                begun.address.as_deref(),
            );
            Ok(Applied {
                rev: plan.result_rev.clone(),
                applied: plan.applies.clone(),
                restart_needed: restart,
                secrets,
                snippets,
                not_listening,
            })
        }
    }
}
