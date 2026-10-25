use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry, entity_of_section};
use burrow_ui_kit::{Icon, Kbd};
use leptos::prelude::*;

use crate::route::{self, Route};
use crate::store::{Store, short_rev, store};

#[derive(Clone, PartialEq)]
struct Hit {
    section: &'static str,
    icon: &'static str,
    label: String,
    facet: String,
    go: Route,
    query: Option<(&'static str, &'static str)>,
}

fn marked(label: &str, needle: &str) -> AnyView {
    let lower = label.to_lowercase();
    match (!needle.is_empty()).then(|| lower.find(needle)).flatten() {
        Some(at) if label.is_char_boundary(at) && label.is_char_boundary(at + needle.len()) => {
            let (a, rest) = label.split_at(at);
            let (b, c) = rest.split_at(needle.len());
            view! { <span>{a.to_string()}<span class="font-bold text-accent-ink">{b.to_string()}</span>{c.to_string()}</span> }.into_any()
        }
        _ => view! { <span>{label.to_string()}</span> }.into_any(),
    }
}

fn search_hits(s: Store, needle: &str) -> Vec<Hit> {
    let doc = s.edit.doc();
    let status = s.status.get_untracked();
    let mut out = Vec::new();
    if s.can_write_now() {
        action_hits(s, &doc, needle, &mut out);
    }
    entity_hits(&doc, status.as_deref(), needle, &mut out);
    setting_hits(s, needle, &mut out);
    history_hits(s, needle, &mut out);
    for (r, l, i) in [
        (Route::Settings, "Settings", "settings"),
        (Route::Activity(None), "Activity", "activity"),
        (Route::About, "About", "about"),
    ] {
        if needle.is_empty() || l.to_lowercase().contains(needle) {
            out.push(Hit {
                section: "Pages",
                icon: i,
                label: l.to_string(),
                facet: String::new(),
                go: r,
                query: None,
            });
        }
    }
    out.truncate(14);
    out
}

fn hit(needle: &str, l: &str) -> bool {
    needle.is_empty() || l.to_lowercase().contains(needle)
}

fn names(doc: &Value, sec: &str) -> Vec<String> {
    doc.get(sec)
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default()
}

fn action_hits(s: Store, doc: &Value, needle: &str, out: &mut Vec<Hit>) {
    for c in names(doc, ClientEntry::section()) {
        let label = format!("Rotate secret of {c}…");
        if !needle.is_empty() && hit(needle, &label) {
            out.push(Hit {
                section: "Actions",
                icon: "refresh",
                label,
                facet: ClientEntry::ENTITY.into(),
                go: Route::Detail(ClientEntry::section().into(), c),
                query: Some(("tab", "secrets")),
            });
        }
    }
    for (sec, what) in [
        (ServiceEntry::section(), ServiceEntry::ENTITY),
        (ClientEntry::section(), ClientEntry::ENTITY),
        (VisitorEntry::section(), VisitorEntry::ENTITY),
    ] {
        let label = format!("New {what}…");
        if hit(needle, &label) {
            out.push(Hit {
                section: "Actions",
                icon: "plus",
                label,
                facet: String::new(),
                go: Route::List(sec.into()),
                query: Some(("new", "1")),
            });
        }
    }
    let n = s.edit.pending();
    if n > 0 {
        let label = format!("Review {n} change{}", if n == 1 { "" } else { "s" });
        if hit(needle, &label) {
            out.push(Hit {
                section: "Actions",
                icon: "diff",
                label,
                facet: String::new(),
                go: Route::Review,
                query: None,
            });
        }
    }
}

fn entity_hits(doc: &Value, status: Option<&Status>, needle: &str, out: &mut Vec<Hit>) {
    for sec in [
        ServiceEntry::section(),
        ClientEntry::section(),
        VisitorEntry::section(),
    ] {
        let (title, icon) = (
            crate::screens::plural(sec),
            crate::screens::entity_icon(sec),
        );
        for n in names(doc, sec) {
            if !hit(needle, &n) {
                continue;
            }
            let v = doc
                .get(sec)
                .and_then(|m| m.get(&n))
                .cloned()
                .unwrap_or(Value::Null);
            let facet = entity_facet(sec, &n, &v, status);
            out.push(Hit {
                section: title,
                icon,
                label: n.clone(),
                facet,
                go: Route::Detail(sec.into(), n),
                query: None,
            });
        }
    }
}

fn entity_facet(sec: &str, n: &str, v: &Value, status: Option<&Status>) -> String {
    match entity_of_section(sec) {
        Some(Entity::Client) => {
            let c = status
                .as_ref()
                .and_then(|st| st.clients.iter().find(|c| c.name == n))
                .cloned();
            let secrets = match ClientEntry::SECRET.get(v) {
                Some(Value::Array(a)) => a.len(),
                Some(_) => 1,
                None => 0,
            };
            match c {
                Some(c) if c.connected => format!(
                    "{} · {secrets} secret{}",
                    c.from
                        .unwrap_or_default()
                        .rsplit_once(':')
                        .map(|(h, _)| h.to_string())
                        .unwrap_or_default(),
                    if secrets == 1 { "" } else { "s" }
                ),
                _ => format!(
                    "offline · {secrets} secret{}",
                    if secrets == 1 { "" } else { "s" }
                ),
            }
        }
        Some(Entity::Service) => {
            burrow_admin_logic::web::exposure(v).unwrap_or_else(|| "private".into())
        }
        _ => VisitorEntry::ADMIN
            .get(v)
            .map(|a| format!("admin · {}", crate::form::plain(a)))
            .unwrap_or_else(|| "visitor".into()),
    }
}

fn setting_hits(s: Store, needle: &str, out: &mut Vec<Hit>) {
    if let Some(schema) = s.schema.get_untracked() {
        let skip: Vec<String> = schema
            .entities
            .iter()
            .filter_map(|e| e.path.first().cloned())
            .collect();
        for f in burrow_admin_logic::tree::fields(&schema.root)
            .iter()
            .filter(|f| !skip.contains(&f.key))
        {
            if !needle.is_empty() && (hit(needle, &f.node.label) || hit(needle, &f.key)) {
                out.push(Hit {
                    section: "Settings",
                    icon: "settings",
                    label: f.node.label.clone(),
                    facet: f.key.clone(),
                    go: Route::Settings,
                    query: Some((
                        "tab",
                        burrow_admin_logic::settings_tabs::tab_of(&f.key).key(),
                    )),
                });
            }
        }
    }
}

fn history_hits(s: Store, needle: &str, out: &mut Vec<Hit>) {
    if !needle.is_empty() {
        for e in s.history.get_untracked().unwrap_or_default() {
            let summary = e
                .changes
                .first()
                .map(|c| c.summary.clone())
                .unwrap_or_default();
            if hit(needle, &summary) {
                out.push(Hit {
                    section: "Activity",
                    icon: "history",
                    label: format!("rev {} · {summary}", short_rev(&e.rev)),
                    facet: crate::format::relative(e.at),
                    go: Route::Activity(Some(e.id.to_string())),
                    query: None,
                });
            }
        }
    }
}

#[component]
pub(super) fn SearchPopover() -> impl IntoView {
    let s = store();
    let q = RwSignal::new(String::new());
    let active = RwSignal::new(0usize);
    let input: NodeRef<leptos::html::Input> = NodeRef::new();
    Effect::new(move |_| {
        if s.search_open.get() {
            q.set(String::new());
            active.set(0);
            if let Some(i) = input.get() {
                let _ = i.focus();
            }
        }
    });
    let hits = Memo::new(move |_| {
        if !s.search_open.get() {
            return Vec::new();
        }
        search_hits(s, &q.get().trim().to_lowercase())
    });
    let run = move |h: Hit| {
        s.search_open.set(false);
        route::go(h.go.clone());
        if let Some((k, v)) = h.query {
            route::set_query(k, v);
            if k == "new" {
                s.creating.set(Some(h.go.section().to_string()));
            }
        }
    };
    let body = move || {
        let list = hits.get();
        let needle = q.get().trim().to_lowercase();
        let act = active.get().min(list.len().saturating_sub(1));
        let mut last = "";
        list.into_iter()
            .enumerate()
            .map(|(i, h)| {
                let head = (h.section != last).then(|| view! { <li class="px-2.5 pt-2.5 pb-1 text-2xs font-bold tracking-[0.08em] text-text-faint uppercase" role="presentation">{h.section}</li> });
                last = h.section;
                let h2 = h.clone();
                view! {
                    {head}
                    <li role="option" aria-selected=(i == act).to_string()>
                        <button
                            type="button"
                            class="flex w-full cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 text-left text-sm"
                            class:bg-surface-3=i == act
                            on:mouseenter=move |_| active.set(i)
                            on:click=move |_| run(h2.clone())
                        >
                            <span class="text-text-muted"><Icon name=h.icon/></span>
                            {marked(&h.label, &needle)}
                            <span class="ml-auto truncate text-xs text-text-muted">{h.facet.clone()}</span>
                            {(i == act).then(|| view! { <Kbd text="↵"/> })}
                        </button>
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <Show when=move || s.search_open.get()>
            <div class="fixed inset-0 z-40 bg-scrim" on:click=move |_| s.search_open.set(false)></div>
            <div
                class="fixed top-[84px] left-1/2 z-50 w-[min(640px,calc(100vw-32px))] -translate-x-1/2 overflow-hidden rounded-dialog border border-border-strong bg-surface shadow-overlay dark:bg-surface-2"
                role="dialog"
                aria-modal="true"
                aria-label="Search"
                data-testid="search"
                on:keydown=move |e| match e.key().as_str() {
                    "Escape" => s.search_open.set(false),
                    "ArrowDown" => {
                        e.prevent_default();
                        let n = hits.with(Vec::len);
                        if n > 0 {
                            active.update(|a| *a = (*a + 1).min(n - 1));
                        }
                    }
                    "ArrowUp" => {
                        e.prevent_default();
                        active.update(|a| *a = a.saturating_sub(1));
                    }
                    "Enter" => {
                        e.prevent_default();
                        if let Some(h) = hits.with(|l| l.get(active.get_untracked()).cloned()) {
                            run(h);
                        }
                    }
                    _ => {}
                }
            >
                <div class="flex items-center gap-2.5 border-b border-border px-4">
                    <Icon name="search"/>
                    <input
                        node_ref=input
                        type="text"
                        role="combobox"
                        aria-expanded="true"
                        aria-label="Search services, clients, visitors, settings and history"
                        placeholder="Search or run an action"
                        class="h-[50px] flex-1 bg-transparent text-base outline-none placeholder:text-text-faint"
                        prop:value=move || q.get()
                        on:input:target=move |e| {
                            q.set(e.target().value());
                            active.set(0);
                        }
                    />
                    <Kbd text="esc"/>
                </div>
                <ul class="max-h-[60vh] overflow-y-auto p-1.5" role="listbox">{body}</ul>
                <div class="flex items-center gap-3 border-t border-border px-4 py-2.5 text-xs text-text-muted">
                    <span>"↑↓ move"</span>
                    <span>"↵ run"</span>
                    <span class="ml-auto">"Searches services, clients, visitors, settings and history"</span>
                </div>
            </div>
        </Show>
    }
}
