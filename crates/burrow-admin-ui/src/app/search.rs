use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry, entity_of_section};
use burrow_ui_kit::{Icon, Kbd, Modal, PickHead, PickOption, marked, word_at};
use leptos::prelude::*;

use crate::route::{self, Route};
use crate::store::{Store, store};

#[derive(Clone, PartialEq)]
struct Hit {
    section: &'static str,
    icon: &'static str,
    label: String,
    facet: String,
    go: Route,
    query: Option<(&'static str, &'static str)>,
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
        if !s.can_write_item(&[ClientEntry::section().into(), c.clone()]) {
            continue;
        }
        let label = format!("Rotate secret of {c}…");
        if !needle.is_empty() && word_at(&label.to_lowercase(), needle).is_some() {
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
        let label = format!(
            "Review {}",
            burrow_admin_logic::wording::count_of(n, "change")
        );
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
            let c = burrow_admin_logic::presence::of(status, n);
            let secrets = match ClientEntry::SECRET.get(v) {
                Some(Value::Array(a)) => a.len(),
                Some(_) => 1,
                None => 0,
            };
            match c {
                Some(c) if c.online => format!(
                    "{} · {}",
                    c.from.unwrap_or_default(),
                    burrow_admin_logic::wording::count_of(secrets, ClientEntry::SECRET.key())
                ),
                _ => format!(
                    "{} · {}",
                    burrow_admin_logic::wording::OFFLINE,
                    burrow_admin_logic::wording::count_of(secrets, ClientEntry::SECRET.key())
                ),
            }
        }
        Some(Entity::Service) => burrow_admin_logic::web::reached(v),
        _ => VisitorEntry::ADMIN
            .get(v)
            .map(|a| format!("admin · {}", crate::form::plain(a)))
            .unwrap_or_else(|| "visitor".into()),
    }
}

fn setting_hits(s: Store, needle: &str, out: &mut Vec<Hit>) {
    if let Some(schema) = s.schema.get_untracked() {
        let skip = burrow_admin_logic::tree::entity_sections(&schema);
        let skipped: Vec<&str> = skip.iter().map(String::as_str).collect();
        let cards = crate::form::settings_cards(&schema.root, &skipped);
        for f in burrow_admin_logic::tree::fields(&schema.root)
            .iter()
            .filter(|f| !skip.contains(&f.key))
        {
            let label = cards
                .iter()
                .find(|c| c.object && c.at.is_empty() && c.fields.iter().any(|g| g.key == f.key))
                .map_or_else(|| f.node.label.clone(), |c| c.title.clone());
            if !needle.is_empty()
                && (hit(needle, &label) || hit(needle, &f.node.label) || hit(needle, &f.key))
            {
                out.push(Hit {
                    section: "Settings",
                    icon: "settings",
                    label,
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
                    label: format!("{} · {summary}", s.version(&e.rev)),
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
        let new = h
            .query
            .is_some_and(|(k, _)| k == "new")
            .then(|| h.go.section().to_string());
        route::go_then(s, h.go.clone(), h.query, move || {
            if let Some(sec) = new.clone() {
                s.creating.set(Some(sec));
            }
        });
    };
    let body = move || {
        let list = hits.get();
        let needle = q.get().trim().to_lowercase();
        let act = active.get().min(list.len().saturating_sub(1));
        if list.is_empty() {
            return view! { <li class="px-2.5 py-3 text-sm text-text-muted" role="presentation">"Nothing matches"</li> }.into_any();
        }
        let mut last = "";
        list.into_iter()
            .enumerate()
            .map(|(i, h)| {
                let head = (h.section != last).then(|| view! { <PickHead text=h.section/> });
                last = h.section;
                let h2 = h.clone();
                let label = marked(&h.label, &needle);
                view! {
                    {head}
                    <PickOption
                        icon=h.icon
                        active=Signal::derive(move || i == act)
                        on_hover=Callback::new(move |_| active.set(i))
                        on_pick=Callback::new(move |_| run(h2.clone()))
                        note=h.facet.clone()
                    >
                        {label}
                    </PickOption>
                }
            })
            .collect_view()
            .into_any()
    };
    let keys = Callback::new(move |e: web_sys::KeyboardEvent| match e.key().as_str() {
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
    });
    let close = Callback::new(move |_: ()| s.search_open.set(false));
    view! {
        <Modal
            open=s.search_open
            on_close=close
            label="Search".to_string()
            class="fixed top-[84px] left-1/2 z-50 w-[min(640px,calc(100vw-32px))] -translate-x-1/2 overflow-hidden rounded-dialog border border-border-strong bg-surface shadow-overlay dark:bg-surface-2"
            testid="search"
            on_key=keys
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
                    <span>"Enter run"</span>
                    <span class="ml-auto">"Searches services, clients, visitors, settings and history"</span>
                </div>
        </Modal>
    }
}
