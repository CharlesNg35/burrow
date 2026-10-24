use burrow_admin_proto::schema::Schema;
use burrow_ui_kit::Tone;
use leptos::context::Provider;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::edit::Edit;
use crate::form::{PageSearch, ShowKeys, card_fields, settings_card, settings_cards};
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

pub fn groups(
    schema: &Schema,
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
            if is_changed {
                tags.push(("changed".to_string(), Tone::Accent));
            }
            if n_locked > 0 {
                let all = n_locked == inside.len();
                tags.push((
                    if all {
                        "locked".to_string()
                    } else {
                        format!("{n_locked} locked")
                    },
                    Tone::Neutral,
                ));
            }
            if needs_restart {
                tags.push(("restart needed".to_string(), Tone::Info));
            }
            Row {
                key: c.key.clone(),
                to: Route::Settings,
                icon: "settings",
                title: c.title.clone(),
                line2: String::new(),
                tags,
                filters,
                terms: inside
                    .iter()
                    .map(|(p, f)| format!("{} {}", f.node.label, p.join(".")))
                    .collect::<Vec<_>>()
                    .join("\n"),
                ..Row::default()
            }
        })
        .collect()
}

pub fn setting_errors(diags: &[burrow_admin_proto::diag::Diag]) -> usize {
    diags
        .iter()
        .filter(|d| {
            d.is_error()
                && d.path
                    .as_ref()
                    .and_then(|p| p.first())
                    .is_some_and(|k| !crate::route::SECTIONS.contains(&k.as_str()))
        })
        .count()
}

#[component]
pub fn Settings() -> impl IntoView {
    let s = store();
    let current: RwSignal<Option<String>> = RwSignal::new(None);
    let query = RwSignal::new(String::new());
    let rows = Memo::new(move |_| {
        let schema = s.schema.get();
        let restart = s
            .state
            .with(|st| st.as_ref().map(|st| st.runtime.restart_pending.clone()))
            .unwrap_or_default();
        let changed = |p: &[String]| s.edit.draft.with(|d| d.modified(p));
        schema
            .map(|sc| groups(&sc, &changed, &restart))
            .unwrap_or_default()
    });
    let count = Signal::derive(move || rows.with(Vec::len).to_string());
    let list = settings_list(rows, count, current, query);
    let body = move || {
        let schema = s.schema.get()?;
        let skip = sections(&schema);
        let skip: Vec<&str> = skip.iter().map(String::as_str).collect();
        let cards = settings_cards(&schema.root, &skip);
        Some(
            view! { <div class="max-w-[1120px]">{cards.iter().map(settings_card).collect_view()}</div> },
        )
    };
    let e = s.edit;
    let root: Vec<String> = Vec::new();
    let editing = {
        let root = root.clone();
        Memo::new(move |_| e.is_editing(&root))
    };
    let changed = changed_fields(e);
    let problems = problem_count(e);
    let start = move |_| e.start_edit(&[]);
    let close = move || discard_settings(e);
    let save = move || {
        e.stop_edit(&[]);
        s.toast(
            burrow_ui_kit::Tone::Success,
            "Added to changes",
            Some("Review"),
        );
    };
    let can = s.can_write_now();
    let detail = view! {
        <Provider value=ShowKeys(true)>
        <Provider value=PageSearch(query)>
        <Provider value=crate::form::EditMode(Signal::derive(move || editing.get()))>
        <DetailPane>
            <header class="mb-[22px] flex flex-wrap items-start gap-3.5">
                <div class="min-w-0 flex-1">
                    <p class="mb-1.5 text-[12.5px] font-semibold">"Settings"</p>
                    <h1 class="text-[25px] font-bold tracking-[-0.015em]">"Server settings"</h1>
                    <p class="mt-1 text-base text-text-muted">"Everything in server.yaml besides clients, visitors and services. Each field says when it takes effect."</p>
                </div>
                {move || (!editing.get()).then(|| view! {
                    <burrow_ui_kit::Button label="Edit" icon="edit" testid="edit" disabled=!can reason=if can { String::new() } else { "Editing needs write admin".to_string() } on_click=start/>
                })}
            </header>
            <div data-testid="settings">{body}</div>
            {move || editing.get().then(|| {
                let bad = problems.get() > 0;
                view! {
                    <div class=format!("sticky bottom-4 z-20 mt-6 flex items-center gap-3 rounded-card border bg-surface px-4 py-3 shadow-overlay {}", if bad { "border-danger" } else { "border-border" }) data-testid="save-bar" data-sticky-bar="">
                        <span class="text-sm text-text-muted" data-testid="fields-changed">{move || {
                            let n = changed.get();
                            if bad { format!("{} problem{} to fix", problems.get(), if problems.get() == 1 { "" } else { "s" }) } else if n == 0 { "No changes yet".to_string() } else { format!("{n} field{} changed", if n == 1 { "" } else { "s" }) }
                        }}</span>
                        <span class="ml-auto"></span>
                        <burrow_ui_kit::Button label=Signal::derive(move || if changed.get() == 0 { "Close".to_string() } else { "Discard".to_string() }) variant="ghost" testid="discard-edit" on_click=move |_| close()/>
                        <burrow_ui_kit::Button label="Add to changes" variant="primary" testid="save" disabled=Signal::derive(move || bad || changed.get() == 0) reason=Signal::derive(move || if bad { "Fix the problems first".to_string() } else if changed.get() == 0 { "Nothing changed yet".to_string() } else { String::new() }) on_click=move |_| save()/>
                    </div>
                }
            })}
        </DetailPane>
        </Provider>
        </Provider>
        </Provider>
    }
    .into_any();
    master_detail(list, detail)
}

fn settings_list(
    rows: Memo<Vec<Row>>,
    count: Signal<String>,
    current: RwSignal<Option<String>>,
    query: RwSignal<String>,
) -> AnyView {
    view! {
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
                compact=true
                query=query
            />
        </div>
    }
    .into_any()
}

fn settings_of(
    doc: &burrow_admin_proto::serde_json::Value,
) -> burrow_admin_proto::serde_json::Value {
    let mut out = burrow_admin_proto::serde_json::Map::new();
    if let Some(m) = doc.as_object() {
        for (k, v) in m {
            if !crate::route::SECTIONS.contains(&k.as_str()) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    burrow_admin_proto::serde_json::Value::Object(out)
}

fn changed_fields(e: Edit) -> Signal<usize> {
    Signal::derive(move || {
        let now = settings_of(&e.doc());
        e.editing.with(|m| {
            let snap = m
                .get(&Vec::<String>::new())
                .cloned()
                .flatten()
                .map(|d| settings_of(&d))
                .unwrap_or_else(|| now.clone());
            burrow_admin_logic::changeset::ops_between(&snap, &now).len()
        })
    })
}

fn problem_count(e: Edit) -> Signal<usize> {
    Signal::derive(move || {
        let local = e.local.with(|l| {
            l.keys()
                .filter(|p| {
                    p.first()
                        .is_some_and(|k| !crate::route::SECTIONS.contains(&k.as_str()))
                })
                .count()
        });
        let server = e
            .plan
            .with(|p| p.as_ref().map_or(0, |p| setting_errors(&p.diags)));
        local.max(server)
    })
}

fn discard_settings(e: Edit) {
    let snap = e
        .editing
        .with_untracked(|m| m.get(&Vec::<String>::new()).cloned().flatten());
    if let Some(snap) = snap {
        let now = e.doc();
        let mut keys: Vec<String> = Vec::new();
        for d in [&snap, &now] {
            if let Some(m) = d.as_object() {
                keys.extend(
                    m.keys()
                        .filter(|k| !crate::route::SECTIONS.contains(&k.as_str()))
                        .cloned(),
                );
            }
        }
        keys.sort();
        keys.dedup();
        for k in keys {
            let (a, b) = (snap.get(&k).cloned(), now.get(&k).cloned());
            if a != b {
                e.set(std::slice::from_ref(&k), a);
            }
        }
        e.local.update(|l| {
            l.retain(|p, _| {
                p.first()
                    .is_some_and(|k| crate::route::SECTIONS.contains(&k.as_str()))
            })
        });
    }
    e.stop_edit(&[]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::diag::{Diag, Severity};

    fn diag(severity: Severity, path: &[&str]) -> Diag {
        Diag {
            severity,
            code: "x".into(),
            message: "m".into(),
            path: Some(path.iter().map(|s| s.to_string()).collect()),
            also: Vec::new(),
            label: None,
            help: None,
            line: None,
            col: None,
            before: false,
        }
    }

    #[test]
    fn a_server_error_in_a_setting_blocks_adding_the_settings_to_the_changes() {
        let diags = [
            diag(Severity::Error, &["pool", "min_idle"]),
            diag(Severity::Warning, &["pool", "max_idle"]),
            diag(Severity::Error, &["services", "web", "bind"]),
        ];
        assert_eq!(setting_errors(&diags), 1);
        assert_eq!(setting_errors(&diags[1..]), 0);
    }
}
