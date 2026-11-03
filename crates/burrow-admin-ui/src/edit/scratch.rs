use burrow_admin_logic::changeset::Changeset;
use burrow_admin_proto::Path;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value;
use leptos::prelude::*;

use super::Edit;

pub const NEW: &str = "+new";

#[derive(Debug, Clone, PartialEq)]
pub struct Scratch {
    pub at: Path,
    pub value: Value,
}

fn put(base: Value, at: &[String], v: Option<Value>) -> Value {
    let mut c = Changeset::new(base);
    match v {
        Some(v) if at.is_empty() => return v,
        Some(v) => c.set(at, v),
        None if at.is_empty() => return Value::Null,
        None => c.remove_pruned(at),
    }
    c.draft().clone()
}

pub fn placed(doc: &Value, all: &[Scratch]) -> Value {
    all.iter()
        .fold(doc.clone(), |d, s| put(d, &s.at, Some(s.value.clone())))
}

impl Edit {
    pub fn start_scratch(&self, at: Path, start: Value) {
        self.forget_local(&at);
        self.scratch.update(|all| {
            all.retain(|s| s.at != at);
            all.push(Scratch { at, value: start });
        });
    }

    pub fn take_scratch(&self, at: &[String]) -> Option<Value> {
        let found = self
            .scratch
            .with_untracked(|all| all.iter().find(|s| s.at == at).map(|s| s.value.clone()))?;
        self.scratch.update(|all| all.retain(|s| s.at != at));
        self.forget_local(at);
        Some(found)
    }

    fn forget_local(&self, at: &[String]) {
        self.local.update(|l| l.retain(|p, _| !p.starts_with(at)));
    }

    pub(super) fn scratch_get(&self, path: &[String], tracked: bool) -> Option<Option<Value>> {
        let read = |all: &Vec<Scratch>| {
            let s = all.iter().find(|s| path.starts_with(&s.at))?;
            Some(value::get(&s.value, &path[s.at.len()..]).cloned())
        };
        if tracked {
            self.scratch.with(read)
        } else {
            self.scratch.with_untracked(read)
        }
    }

    pub(super) fn scratch_set(&self, path: &[String], v: Option<Value>) -> bool {
        let inside = self
            .scratch
            .with_untracked(|all| all.iter().any(|s| path.starts_with(&s.at)));
        if inside {
            self.scratch.update(|all| {
                if let Some(s) = all.iter_mut().find(|s| path.starts_with(&s.at)) {
                    s.value = put(std::mem::take(&mut s.value), &path[s.at.len()..], v);
                }
            });
        }
        inside
    }

    pub fn with_doc<T>(&self, f: impl FnOnce(&Value) -> T) -> T {
        let all = self.scratch.get();
        self.draft.with(|d| {
            if all.is_empty() {
                f(d.draft())
            } else {
                f(&placed(d.draft(), &all))
            }
        })
    }
}
