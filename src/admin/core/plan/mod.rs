mod apply;
mod locks;
mod refs;
mod steps;

use std::collections::BTreeSet;

use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::msg::ErrorCode;
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{ApplyHow, Plan};
use burrow_admin_proto::schema::{Applies, SemType};
use serde_json::Value;

use crate::config::settings::ServerSettings;
use crate::config::{Loaded, Role, load::load_str_in, patch, tree};

use super::diags::Mapper;
use super::ops::{self, Fresh};
use super::{ApiError, Core, Session, text};
pub use apply::{Begun, ReloadResult, apply_begin, finish, restore};
use locks::{refuse_changed_locks, refuse_locked};
use refs::refuse_new_references;
use steps::{Draft, assemble, conflict, judge, new_document, parse_current};

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
    refuse_changed_locks(&core.schema, (&raw, &new_raw), &applied.renames)?;
    refuse_new_references((&raw, &new_raw), &applied.renames)?;
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
