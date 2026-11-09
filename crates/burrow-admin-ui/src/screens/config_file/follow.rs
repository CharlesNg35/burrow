use std::time::Duration;

use config_editor_leptos::{DocumentsHandle, EditorHandle, FieldPath, TextPosition, TextRange};
use leptos::prelude::*;

use super::problems::field;
use crate::edit::Spot;

const TRIES_MS: [u64; 5] = [0, 50, 150, 400, 1000];

fn reveal(e: &EditorHandle, spot: &Spot) -> bool {
    match spot {
        Spot::Here => {
            e.focus();
            true
        }
        Spot::Path(p) => e.reveal(field(p)),
        Spot::Pointer(p) => e.reveal(FieldPath::from_pointer(p)),
        Spot::Range(line, col, end) => e.reveal_range(TextRange::new(
            TextPosition::new(*line as usize, *col as usize),
            TextPosition::new(*line as usize, *end as usize),
        )),
    }
}

pub(super) fn follow(
    active: RwSignal<String>,
    spot: RwSignal<Option<(String, Spot)>>,
    tabs: DocumentsHandle,
) {
    Effect::new(move |_| {
        let name = active.get();
        if name.is_empty() {
            return;
        }
        for ms in TRIES_MS {
            let name = name.clone();
            set_timeout(
                move || {
                    let shown = untrack(|| tabs.active());
                    if active.with_untracked(|a| *a == name) && shown.as_deref() != Some(&name) {
                        tabs.show(&name);
                    }
                },
                Duration::from_millis(ms),
            );
        }
    });
    Effect::new(move |_| {
        let Some(want) = spot.get() else {
            return;
        };
        for ms in TRIES_MS {
            let want = want.clone();
            set_timeout(
                move || {
                    if spot.try_with_untracked(|s| s.as_ref() != Some(&want)) != Some(false) {
                        return;
                    }
                    let (name, at) = &want;
                    let mounted = tabs.editor(name).filter(EditorHandle::is_mounted);
                    let done = mounted.is_some_and(|e| {
                        reveal(&e, at) || {
                            e.focus();
                            true
                        }
                    });
                    if done || ms == TRIES_MS[TRIES_MS.len() - 1] {
                        spot.set(None);
                    }
                },
                Duration::from_millis(ms),
            );
        }
    });
}
