use burrow_admin_logic::files;
use burrow_admin_proto::Path;
use burrow_ui_kit::{FILE_TAG_WIDTH, FileName, Icon, LockLine, MENU_ITEM, Tone, tag_class};
use leptos::prelude::*;

use crate::store::store;

type ByItem = std::rc::Rc<std::collections::HashMap<Path, (String, bool)>>;
type Seen = std::sync::Arc<burrow_admin_proto::msg::State>;

thread_local! {
    static BY_ITEM: std::cell::RefCell<(Option<Seen>, ByItem)> = std::cell::RefCell::default();
}

#[derive(Default)]
pub(crate) struct Files {
    on: bool,
    several: bool,
    pub main: String,
    by_item: ByItem,
}

fn by_item(info: &burrow_admin_proto::msg::FileInfo, at: Option<Seen>) -> ByItem {
    if let Some(kept) = BY_ITEM.with(|k| {
        let k = k.borrow();
        let same = matches!((&k.0, &at), (Some(a), Some(b)) if std::sync::Arc::ptr_eq(a, b));
        same.then(|| k.1.clone())
    }) {
        return kept;
    }
    let mut map = std::collections::HashMap::new();
    for f in &info.files {
        for item in &f.items {
            map.entry(item.clone())
                .or_insert_with(|| (f.name.clone(), f.read_only.is_some()));
        }
    }
    let map = std::rc::Rc::new(map);
    BY_ITEM.with(|k| *k.borrow_mut() = (at, map.clone()));
    map
}

impl Files {
    pub fn now() -> Files {
        let s = store();
        let at = s.state.get();
        let (main, by_item) = s
            .with_files(|i| (files::main_name(i), by_item(i, at)))
            .unwrap_or_default();
        Files {
            on: s.files_on(),
            several: several(),
            main,
            by_item,
        }
    }

    pub fn item_file(&self, item: &[String], was: Option<&Path>) -> Option<String> {
        if !self.on {
            return None;
        }
        let placed = store()
            .edit
            .files
            .with(|f| item.get(..2).and_then(|i| f.placed(i)).map(str::to_string));
        placed.or_else(|| match was {
            Some(w) => Some(
                self.by_item
                    .get(w)
                    .map_or_else(|| self.main.clone(), |f| f.0.clone()),
            ),
            None => Some(self.main.clone()),
        })
    }

    pub fn label(&self, item: &[String], was: Option<&Path>) -> Option<(String, bool)> {
        if !self.several {
            return None;
        }
        let name = self.item_file(item, was)?;
        let locked = was.is_some_and(|w| self.by_item.get(w).is_some_and(|f| f.1));
        Some((name, locked))
    }
}

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
        let class = format!("{} {FILE_TAG_WIDTH}", tag_class(Tone::Neutral));
        Some(
            view! {
                <span class=class data-testid="file-tag" title=title>
                    {locked.then(|| view! { <Icon name="lock" small=true/> })}
                    <FileName name=name class="font-mono"/>
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
    let Some(((main, own), base)) = s
        .with_files(|i| {
            (
                files::main_name(i),
                files::own_file(i, path.get(1).map_or("", String::as_str)),
            )
        })
        .zip(s.base_file(path))
    else {
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
    let base = s.base_file(&path).unwrap_or_default();
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
