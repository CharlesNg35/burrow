use burrow_admin_proto::schema::{Field, Schema, SemType};
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::get;
use burrow_ui_kit::Tone;
use leptos::context::Provider;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::form::{ShowKeys, card_fields, locked_text, settings_card, settings_cards};
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

fn short(f: &Field, v: Option<&Value>) -> Option<String> {
    let shown = v.filter(|v| !v.is_null()).or(f.node.default.as_ref())?;
    match shown {
        Value::Object(_) => None,
        Value::Array(a) if a.is_empty() => None,
        other => Some(locked_text(&f.node, other)),
    }
}

pub fn groups(
    schema: &Schema,
    values: &Value,
    changed: &dyn Fn(&[String]) -> bool,
    restart: &[String],
) -> Vec<Row> {
    let skip = sections(schema);
    let skip: Vec<&str> = skip.iter().map(String::as_str).collect();
    let locked = schema.locked_paths();
    settings_cards(&schema.root, &skip)
        .into_iter()
        .map(|c| {
            let inside = card_fields(&c);
            let n = inside.len();
            let unset = c.object
                && c.fields.iter().any(|f| {
                    matches!(f.node.sem, SemType::Optional(_))
                        && get(values, std::slice::from_ref(&f.key)).is_none_or(Value::is_null)
                });
            let summary = if unset {
                "off".to_string()
            } else {
                inside
                    .iter()
                    .filter_map(|(p, f)| short(f, get(values, p)))
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            let tops: Vec<String> = c.fields.iter().map(|f| f.key.clone()).collect();
            let n_locked = inside
                .iter()
                .filter(|(p, _)| locked.iter().any(|l| p.starts_with(l)))
                .count();
            let is_changed = tops.iter().any(|k| changed(std::slice::from_ref(k)));
            let needs_restart = restart
                .iter()
                .any(|r| tops.iter().any(|k| r.split('.').next() == Some(k.as_str())));
            let mut filters = Vec::new();
            if needs_restart {
                filters.push("restart");
            }
            if is_changed {
                filters.push("changed");
            }
            let mut tags = Vec::new();
            if n_locked > 0 {
                tags.push((format!("{n_locked} locked"), Tone::Neutral));
            }
            if needs_restart {
                tags.push(("restart needed".to_string(), Tone::Info));
            }
            Row {
                key: c.key.clone(),
                to: Route::Settings,
                icon: "settings",
                dot: is_changed.then_some(Tone::Info),
                title: c.title.clone(),
                meta: n.to_string(),
                line2: if summary.is_empty() {
                    "not set".into()
                } else {
                    summary
                },
                line3: format!("{n} field{}", if n == 1 { "" } else { "s" }),
                tags,
                filters,
                ..Row::default()
            }
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
        let restart = s
            .state
            .with(|st| st.as_ref().map(|st| st.runtime.restart_pending.clone()))
            .unwrap_or_default();
        let changed = |p: &[String]| s.edit.draft.with(|d| d.modified(p));
        schema
            .map(|sc| groups(&sc, &doc, &changed, &restart))
            .unwrap_or_default()
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
                    if let Ok(Some(el)) = document().query_selector(&format!("#detail [data-card=\"{key}\"]")) {
                        el.scroll_into_view();
                    }
                }
            }
        >
            <ListPane
                title="Settings"
                count=count
                search_label="Search settings"
                filters=vec![("all", "All".into()), ("restart", "Restart".into()), ("changed", "Changed".into())]
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
        let cards = settings_cards(&schema.root, &skip);
        Some(
            view! { <div class="max-w-[1120px]">{cards.iter().map(settings_card).collect_view()}</div> },
        )
    };
    let detail = view! {
        <Provider value=ShowKeys(true)>
        <DetailPane>
            <header class="mb-[22px]">
                <p class="mb-1.5 text-[12.5px] font-semibold">"Settings"</p>
                <h1 class="text-[25px] font-bold tracking-[-0.015em]">"Server settings"</h1>
                <p class="mt-1 text-base text-text-muted">"Everything in server.yaml besides clients, visitors and services. Each field says when it takes effect."</p>
            </header>
            <div data-testid="settings">{body}</div>
        </DetailPane>
        </Provider>
    }
    .into_any();
    master_detail(list, detail)
}
