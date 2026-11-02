use std::collections::BTreeMap;

use burrow_admin_logic::changeset::Changeset;
use burrow_admin_proto::Path;
use burrow_admin_proto::plan::Conflict;
use burrow_admin_proto::serde_json::Value;
use leptos::prelude::*;

use crate::store::Store;

use super::Edit;

impl Edit {
    pub fn is_editing(&self, entity: &[String]) -> bool {
        self.editing.with(|m| m.contains_key(entity))
    }

    pub fn start_edit(&self, entity: &[String]) {
        if self.editing.with_untracked(|m| m.contains_key(entity)) {
            return;
        }
        let snapshot = self.peek(entity);
        self.editing.update(|m| {
            m.insert(entity.to_vec(), snapshot);
        });
    }

    pub fn keep_open_edits(&self) {
        let all: Vec<Path> = self
            .editing
            .with_untracked(|m| m.keys().cloned().collect::<Vec<Path>>());
        let open: Vec<Path> = all
            .into_iter()
            .filter(|p| untrack(|| self.unsaved(p)))
            .collect();
        for p in open {
            self.stop_edit(&p);
        }
    }

    pub fn stop_edit(&self, entity: &[String]) {
        if self.editing.with_untracked(|m| m.contains_key(entity)) {
            self.editing.update(|m| {
                m.remove(entity);
            });
        }
    }

    pub fn discard_edit(&self, entity: &[String]) {
        let Some(snapshot) = self.editing.with_untracked(|m| m.get(entity).cloned()) else {
            return;
        };
        if self
            .local
            .with_untracked(|l| l.keys().any(|p| p.starts_with(entity)))
        {
            self.local
                .update(|l| l.retain(|p, _| !p.starts_with(entity)));
        }
        if self
            .refused
            .with_untracked(|r| r.iter().any(|p| p.starts_with(entity)))
        {
            self.refused
                .update(|r| r.retain(|p| !p.starts_with(entity)));
        }
        if self
            .texts
            .with_untracked(|t| t.keys().any(|p| p.starts_with(entity)))
        {
            self.texts
                .update(|t| t.retain(|p, _| !p.starts_with(entity)));
        }
        if self.peek(entity) != snapshot {
            self.set(entity, snapshot);
        }
        self.stop_edit(entity);
    }

    pub fn unsaved(&self, entity: &[String]) -> bool {
        self.editing.with(|m| {
            m.get(entity).is_some_and(|snap| {
                self.draft
                    .with(|d| burrow_admin_proto::value::get(d.draft(), entity) != snap.as_ref())
            })
        })
    }

    pub(super) fn clear_changed(&self) {
        if let Some(s) = crate::store::try_store()
            && s.changed.with_untracked(Option::is_some)
        {
            s.changed.set(None);
        }
    }

    pub(super) fn clear_edits(&self) {
        if self.editing.with_untracked(|m| !m.is_empty()) {
            self.editing.set(BTreeMap::new());
        }
    }

    pub(super) fn clear_refused(&self) {
        if self.refused.with_untracked(|r| !r.is_empty()) {
            self.refused.set(std::collections::BTreeSet::new());
        }
    }

    pub fn sync_base(&self, values: &Value, rev: &str) -> bool {
        let reset = self.reset_on_load.get_untracked();
        let known = self.base_rev.get_untracked();
        if !reset && known == rev {
            return false;
        }
        let mut carried = false;
        if reset || !untrack(|| self.dirty()) || known.is_empty() {
            self.draft.set(Changeset::new(values.clone()));
            self.texts.set(BTreeMap::new());
            self.files.set(Default::default());
            self.clear_refused();
            if reset {
                self.local.set(BTreeMap::new());
                self.clear_edits();
                let carry = std::mem::take(&mut *self.carry.write_value());
                carried = !carry.is_empty();
                for (path, v) in carry {
                    let entity: Path = path.iter().take(2).cloned().collect();
                    let entity = if crate::route::SECTIONS
                        .contains(&path.first().map(String::as_str).unwrap_or(""))
                    {
                        entity
                    } else {
                        Vec::new()
                    };
                    self.start_edit(&entity);
                    self.draft.update(|d| match v {
                        Some(v) => d.set(&path, v),
                        None => d.remove(&path),
                    });
                }
            } else {
                self.resnapshot(values);
            }
            self.plan.set(None);
            if self.conflict_revert.with_untracked(Option::is_none) {
                self.end_conflict();
            }
            self.base_rev.set(rev.to_string());
            self.reset_on_load.set(false);
        } else if self
            .conflict
            .with_untracked(|c| c.as_ref().map(|c| c.current.as_str()) != Some(rev))
        {
            if let Some(s) = crate::store::try_store()
                && s.changed_from.with_untracked(|(old, _)| *old != known)
            {
                s.changed_from.set((known, js_sys::Date::now()));
            }
            self.plan.set(None);
            self.revalidate_now();
        }
        carried
    }

    pub fn raise(&self, c: Conflict, show: bool) {
        let new = self
            .conflict
            .with_untracked(|o| o.as_ref().map(|o| &o.current) != Some(&c.current));
        self.conflict.set(Some(c));
        if show || new {
            self.conflict_open.set(true);
        }
    }

    pub fn end_conflict(&self) {
        if self.conflict.with_untracked(Option::is_some) {
            self.conflict.set(None);
        }
        if self.conflict_open.get_untracked() {
            self.conflict_open.set(false);
        }
    }

    fn resnapshot(&self, values: &Value) {
        if self.editing.with_untracked(|m| m.is_empty()) {
            return;
        }
        self.editing.update(|m| {
            for (path, snap) in m.iter_mut() {
                *snap = burrow_admin_proto::value::get(values, path).cloned();
            }
        });
    }

    pub fn rebase(&self, store: Store) {
        let Some(state) = store.state.get_untracked() else {
            return;
        };
        self.draft.update(|d| d.rebase(state.values.clone()));
        self.base_rev.set(state.rev.clone());
        self.end_conflict();
        self.clear_refused();
        store.changed.set(None);
        self.changed();
    }
}
