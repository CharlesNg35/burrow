use burrow_admin_logic::tree::fields;
use burrow_admin_proto::schema::Schema;
use burrow_admin_proto::serde_json::Value;
use burrow_ui_kit::Tone;
use leptos::context::Provider;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::form::{ShowKeys, object_view_skipping, plain};
use crate::route::Route;
use crate::screens::{DetailPane, ListPane, Row, master_detail};
use crate::store::store;

fn sections(schema: &Schema) -> Vec<String> {
    schema
        .entities
        .iter()
        .filter_map(|e| e.path.first().cloned())
        .collect()
}

pub fn groups(schema: &Schema, values: &Value) -> Vec<Row> {
    let skip = sections(schema);
    let all: Vec<_> = fields(&schema.root)
        .iter()
        .filter(|f| !skip.contains(&f.key))
        .collect();
    let mut keys: Vec<(String, String)> = schema
        .root
        .groups
        .iter()
        .map(|g| (g.key.clone(), g.label.clone()))
        .collect();
    for f in &all {
        if !keys.iter().any(|(k, _)| *k == f.node.group) {
            keys.insert(0, (f.node.group.clone(), "General".into()));
        }
    }
    let locked = schema.locked_paths();
    keys.into_iter()
        .filter_map(|(key, label)| {
            let mine: Vec<_> = all.iter().filter(|f| f.node.group == key).collect();
            if mine.is_empty() {
                return None;
            }
            let summary = mine
                .iter()
                .filter_map(|f| {
                    values
                        .get(&f.key)
                        .filter(|v| !v.is_null() && !v.is_object())
                        .map(|v| format!("{} {}", f.key, plain(v)))
                })
                .take(2)
                .collect::<Vec<_>>()
                .join(" · ");
            let n_locked = mine
                .iter()
                .filter(|f| locked.iter().any(|l| l.first() == Some(&f.key)))
                .count();
            let mut tags = Vec::new();
            if n_locked > 0 {
                tags.push((format!("{n_locked} locked"), Tone::Neutral));
            }
            Some(Row {
                key: key.clone(),
                to: Route::Settings,
                icon: "settings",
                dot: None,
                title: if label.is_empty() {
                    "General".into()
                } else {
                    label
                },
                meta: format!(
                    "{} field{}",
                    mine.len(),
                    if mine.len() == 1 { "" } else { "s" }
                ),
                line2: if summary.is_empty() {
                    "defaults".into()
                } else {
                    summary
                },
                tags,
                group: String::new(),
                pending: None,
                filters: Vec::new(),
                actions: Vec::new(),
            })
        })
        .collect()
}

#[component]
pub fn Settings() -> impl IntoView {
    let s = store();
    let current: RwSignal<Option<String>> = RwSignal::new(None);
    let rows = Memo::new(move |_| {
        let schema = s.schema.get();
        let doc = s.edit.doc();
        schema.map(|sc| groups(&sc, &doc)).unwrap_or_default()
    });
    let count = Signal::derive(move || rows.with(Vec::len).to_string());
    let list = view! {
        <div
            class="contents"
            on:click=move |e| {
                let hit = e
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .and_then(|t| t.closest("[data-row]").ok().flatten())
                    .and_then(|el| el.get_attribute("data-row"));
                if let Some(key) = hit {
                    current.set(Some(key.clone()));
                    if let Ok(Some(el)) = document().query_selector(&format!("#detail [data-group=\"{key}\"]")) {
                        el.scroll_into_view();
                    }
                }
            }
        >
            <ListPane
                title="Settings"
                count=count
                search_label="Search settings"
                filters=Vec::new()
                groups=vec![""]
                rows=rows
                selected=current
                testid="settings-list"
            />
        </div>
    }
    .into_any();
    let body = move || {
        let schema = s.schema.get()?;
        let skip = sections(&schema);
        let skip: Vec<&str> = skip.iter().map(String::as_str).collect();
        Some(object_view_skipping(
            &schema.root,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            &skip,
        ))
    };
    let detail = view! {
        <Provider value=ShowKeys(true)>
        <DetailPane>
            <header class="mb-[22px]">
                <h1 class="text-[25px] font-bold tracking-[-0.015em]">"Server settings"</h1>
                <p class="mt-1 text-base text-text-muted">"Server-wide settings from server.yaml. Locked ones change only in the file, followed by a restart."</p>
            </header>
            <div data-testid="settings">{body}</div>
        </DetailPane>
        </Provider>
    }
    .into_any();
    master_detail(list, detail)
}
