use burrow_admin_logic::files;
use burrow_admin_proto::Path;
use burrow_ui_kit::{Icon, LockLine, MENU_ITEM, Tone, tag_class};
use leptos::prelude::*;

use crate::store::store;

fn several() -> bool {
    let s = store();
    s.files_on()
        && (s.with_files(|i| i.files.len() > 1).unwrap_or(false)
            || s.edit.files.with(|f| !f.places.is_empty()))
}

pub(crate) fn file_label(item: &[String]) -> Option<(String, bool)> {
    let s = store();
    if !several() {
        return None;
    }
    let name = s.item_file(item)?;
    Some((name, s.item_locked(item).is_some()))
}

pub(super) fn file_tag(path: Path) -> impl Fn() -> Option<AnyView> + Clone + use<> {
    move || {
        let (name, locked) = file_label(&path)?;
        let title = format!("Kept in {name}");
        Some(
            view! {
                <span class=tag_class(Tone::Neutral) data-testid="file-tag" title=title>
                    {locked.then(|| view! { <Icon name="lock" small=true/> })}
                    <span class="font-mono">{name}</span>
                </span>
            }
            .into_any(),
        )
    }
}

pub(super) fn read_only_note(path: Path) -> impl Fn() -> Option<AnyView> + use<> {
    move || {
        let line = store().item_locked(&path)?;
        Some(view! { <LockLine text=line testid="read-only-file" class="-mt-2 mb-3"/> }.into_any())
    }
}

struct Choice {
    label: String,
    file: String,
    testid: &'static str,
}

fn choices(path: &[String]) -> Vec<Choice> {
    let s = store();
    let Some((main, own, base)) = s.with_files(|i| {
        (
            files::main_name(i),
            files::own_file(i, path.get(1).map_or("", String::as_str)),
            files::file_of(i, path),
        )
    }) else {
        return Vec::new();
    };
    if s.edit.files.with(|f| f.placed(path).is_some()) {
        return vec![Choice {
            label: format!("Keep in {base}"),
            file: base,
            testid: "file-keep",
        }];
    }
    vec![if base == main {
        Choice {
            label: "Split into its own file".into(),
            file: own,
            testid: "file-split",
        }
    } else {
        Choice {
            label: format!("Merge into {main}"),
            file: main,
            testid: "file-merge",
        }
    }]
}

pub(super) fn file_menu(path: Path) -> impl IntoView + use<> {
    let s = store();
    if !s.files_on() {
        return ().into_any();
    }
    let base = s
        .with_files(|i| files::file_of(i, &path))
        .unwrap_or_default();
    choices(&path)
        .into_iter()
        .map(|c| {
            let (path, base) = (path.clone(), base.clone());
            view! {
                <button type="button" class=MENU_ITEM data-testid=c.testid on:click=move |_| {
                    s.edit.place(&path, &c.file, &base);
                    s.added_to_changes();
                }>{c.label}</button>
            }
        })
        .collect_view()
        .into_any()
}
