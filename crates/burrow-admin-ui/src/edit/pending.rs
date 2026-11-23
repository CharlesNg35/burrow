use std::collections::BTreeMap;

use burrow_admin_logic::changeset::{Changeset, Mark};
use burrow_admin_logic::files::FileMoves;
use burrow_admin_proto::Path;
use burrow_admin_proto::serde_json::Value;
use leptos::prelude::*;

use super::Edit;

#[derive(Clone)]
pub(super) struct Counted {
    mark: Mark,
    open: Vec<Path>,
    entities: usize,
}

fn covers(open: &[Path], changed: &[String]) -> bool {
    open.iter().any(|o| {
        if o.is_empty() {
            changed
                .first()
                .is_some_and(|k| !crate::route::SECTIONS.contains(&k.as_str()))
        } else {
            changed.starts_with(o)
        }
    })
}

fn saved_entities(
    draft: RwSignal<Changeset>,
    editing: RwSignal<BTreeMap<Path, Option<Value>>>,
    counted: StoredValue<Option<Counted>>,
) -> usize {
    let open: Vec<Path> = editing.with(|m| m.keys().cloned().collect());
    draft.with(|d| {
        let kept = counted.with_value(|c| {
            let c = c.as_ref().filter(|c| c.open == open)?;
            let since = d.changed_since(c.mark)?;
            since.iter().all(|p| covers(&open, p)).then_some(c.entities)
        });
        let entities = kept.unwrap_or_else(|| {
            let snaps: Vec<(Path, Option<Value>)> = editing.with_untracked(|m| {
                m.iter()
                    .map(|(p, snap)| (p.clone(), snap.clone()))
                    .collect()
            });
            if snaps.is_empty() {
                d.changed_entities()
            } else {
                Edit::saved_of(d, &snaps).changed_entities()
            }
        });
        counted.set_value(Some(Counted {
            mark: d.mark(),
            open,
            entities,
        }));
        entities
    })
}

pub(super) fn pending_count(
    draft: RwSignal<Changeset>,
    editing: RwSignal<BTreeMap<Path, Option<Value>>>,
    texts: RwSignal<BTreeMap<Path, String>>,
    files: RwSignal<FileMoves>,
) -> Memo<usize> {
    let counted = StoredValue::new(None);
    Memo::new(move |_| {
        saved_entities(draft, editing, counted)
            + texts.with(BTreeMap::len)
            + files.with(|f| draft.with(|d| f.counted(d)))
    })
}
