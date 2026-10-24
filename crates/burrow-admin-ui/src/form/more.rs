use burrow_admin_logic::more::{is_set, matches, storage_key, summary};
use burrow_admin_logic::relations::field_state;
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::{Field, Node};
use burrow_ui_kit::Disclosure;
use leptos::prelude::*;

use crate::edit::diags_at;
use crate::store::store;

use super::field_id;

#[derive(Clone, Copy)]
pub struct PageSearch(pub RwSignal<String>);

#[derive(Clone)]
pub(super) struct More {
    pub fields: Vec<(Field, Path)>,
    pub spath: Path,
    pub group: String,
    pub entity: Path,
    pub table: bool,
    pub alone: bool,
    pub grid: &'static str,
}

pub(super) const GRID: &str = "grid grid-cols-1 gap-x-6 gap-y-[22px] pt-[22px] md:grid-cols-2";
pub(super) const ROWS: &str = "divide-y divide-rowline border-t border-rowline";

fn key_of(spath: &[String], group: &str) -> String {
    let (page, rest): (&str, &[String]) = match spath.split_first() {
        Some((first, rest)) if crate::route::SECTIONS.contains(&first.as_str()) => {
            (first.as_str(), rest.get(1..).unwrap_or(&[]))
        }
        _ => ("settings", spath),
    };
    let mut parts: Vec<&str> = rest.iter().map(String::as_str).collect();
    parts.push(group);
    storage_key(page, &parts)
}

fn remembered(key: &str) -> Option<bool> {
    let v = window()
        .local_storage()
        .ok()
        .flatten()?
        .get_item(key)
        .ok()
        .flatten()?;
    Some(v == "open")
}

fn remember(key: &str, open: bool) {
    if let Some(st) = window().local_storage().ok().flatten() {
        let _ = st.set_item(key, if open { "open" } else { "closed" });
    }
}

fn visible_fields(m: &More) -> Memo<Vec<(Node, Path)>> {
    let e = store().edit;
    let (fields, entity) = (m.fields.clone(), m.entity.clone());
    Memo::new(move |_| {
        let Some(schema) = store().schema.get() else {
            return Vec::new();
        };
        e.draft.with(|d| {
            fields
                .iter()
                .filter(|(f, _)| field_state(&f.node, &schema.root, d.draft(), &entity).visible)
                .map(|(f, p)| (f.node.clone(), p.clone()))
                .collect()
        })
    })
}

fn wants_open(m: &More, shown: Memo<Vec<(Node, Path)>>) -> Memo<[bool; 3]> {
    let e = store().edit;
    let search = use_context::<PageSearch>();
    let fields = m.fields.clone();
    Memo::new(move |_| {
        let set = shown.with(|v| {
            v.iter()
                .any(|(n, p)| is_set(n, e.value(p).as_ref()) || e.draft.with(|d| d.modified(p)))
        });
        let wrong = shown.with(|v| {
            v.iter().any(|(_, p)| {
                e.plan.with(|plan| {
                    diags_at(plan.as_deref(), p, true)
                        .iter()
                        .any(|d| d.is_error())
                }) || e.local.with(|l| l.keys().any(|k| k.starts_with(p)))
                    || e.related.with(|r| r.keys().any(|k| k.starts_with(p)))
            })
        });
        let found = search.is_some_and(|s| {
            let q = s.0.get();
            fields.iter().any(|(f, _)| matches(f, &q))
        });
        [set, wrong, found]
    })
}

pub(super) fn more_settings(m: More, rows: Vec<AnyView>) -> AnyView {
    let key = key_of(&m.spath, &m.group);
    let shown = visible_fields(&m);
    let auto = wants_open(&m, shown);
    let open =
        RwSignal::new(auto.get_untracked().contains(&true) || remembered(&key) == Some(true));
    let by_search = RwSignal::new(false);
    Effect::new(move |was: Option<[bool; 3]>| {
        let [set, wrong, found] = auto.get();
        let [was_set, was_wrong, was_found] = was.unwrap_or([false; 3]);
        if ((set && !was_set) || (wrong && !was_wrong)) && !open.get_untracked() {
            open.set(true);
        }
        if found && !was_found && !open.get_untracked() {
            open.set(true);
            by_search.set(true);
        }
        if !found && was_found && by_search.get_untracked() {
            open.set(false);
            by_search.set(false);
        }
        [set, wrong, found]
    });
    let e = store().edit;
    let text = Signal::derive(move || {
        shown.with(|v| {
            let set = v
                .iter()
                .filter(|(n, p)| is_set(n, e.value(p).as_ref()))
                .count();
            summary(set, v.len())
        })
    });
    let none = Memo::new(move |_| shown.with(Vec::is_empty));
    let mut id_path = m.spath.clone();
    id_path.push(if m.group.is_empty() {
        "general".into()
    } else {
        m.group.clone()
    });
    let id = format!("{}-more", field_id(&id_path));
    let k2 = key.clone();
    let on_toggle = Callback::new(move |now: bool| {
        by_search.set(false);
        remember(&k2, now);
    });
    let grid = if m.table { ROWS } else { m.grid };
    let outer = if m.table { "" } else { "contents" };
    view! {
        <div class=outer hidden=move || none.get() data-more=key>
            <Disclosure id=id summary=text open=open on_toggle=on_toggle table=m.table flush=m.alone grid=grid.to_string()>
                {rows}
            </Disclosure>
        </div>
    }
    .into_any()
}

pub fn reveal(el: &web_sys::Element) -> bool {
    let Ok(Some(region)) = el.closest("[data-testid=more-region][hidden]") else {
        return false;
    };
    if let Ok(ev) = web_sys::Event::new("beforematch") {
        let _ = region.dispatch_event(&ev);
    }
    true
}
