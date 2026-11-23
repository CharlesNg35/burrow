use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry, entity_of_section};
use burrow_ui_kit::{Icon, Kbd, Modal, PickHead, PickOption, marked, word_at};
use leptos::prelude::*;

use crate::route::{self, Route};
use crate::store::{Store, store};

const PER_SECTION: usize = 5;
const LIST_ID: &str = "search-list";

#[derive(Clone, PartialEq)]
enum Go {
    To(Route, Option<(&'static str, &'static str)>),
    Find(Route, String),
}

#[derive(Clone, PartialEq)]
struct Hit {
    section: &'static str,
    icon: &'static str,
    label: String,
    facet: String,
    go: Go,
}

fn rank(lower: &str, needle: &str) -> Option<u8> {
    if needle.is_empty() || lower == needle {
        Some(0)
    } else if lower.starts_with(needle) {
        Some(1)
    } else if word_at(lower, needle).is_some() {
        Some(2)
    } else if lower.contains(needle) {
        Some(3)
    } else {
        None
    }
}

fn best<T: Ord + Clone>(mut found: Vec<(u8, usize, T)>, keep: usize) -> Vec<T> {
    if keep == 0 {
        return Vec::new();
    }
    if found.len() > keep {
        found.select_nth_unstable(keep - 1);
        found.truncate(keep);
    }
    found.sort_unstable();
    found.into_iter().map(|(_, _, t)| t).collect()
}

fn search_hits(s: Store, needle: &str) -> Vec<Hit> {
    let status = s.status.get_untracked();
    let mut out = Vec::new();
    s.edit.with_doc(|doc| {
        if s.can_write_now() {
            action_hits(s, doc, needle, &mut out);
        }
        if !needle.is_empty() {
            entity_hits(doc, status.as_deref(), needle, &mut out);
        }
    });
    setting_hits(s, needle, &mut out);
    history_hits(s, needle, &mut out);
    page_hits(s, needle, &mut out);
    out
}

fn page_hits(s: Store, needle: &str, out: &mut Vec<Hit>) {
    let counts = s.edit.with_doc(|doc| {
        crate::route::SECTIONS.map(|sec| {
            doc.get(sec)
                .and_then(Value::as_object)
                .map_or(0, |m| m.len())
        })
    });
    for (sec, n) in crate::route::SECTIONS.into_iter().zip(counts) {
        let label = crate::screens::plural(sec);
        if hit(needle, label) {
            out.push(Hit {
                section: "Pages",
                icon: crate::screens::entity_icon(sec),
                label: label.to_string(),
                facet: n.to_string(),
                go: Go::To(Route::List(sec.into()), None),
            });
        }
    }
    for (r, l, i) in [
        (Route::Settings, "Settings", "settings"),
        (Route::Activity(None), "Activity", "activity"),
        (Route::About, "About", "about"),
    ] {
        if hit(needle, l) {
            out.push(Hit {
                section: "Pages",
                icon: i,
                label: l.to_string(),
                facet: String::new(),
                go: Go::To(r, None),
            });
        }
    }
}

fn hit(needle: &str, l: &str) -> bool {
    needle.is_empty() || l.to_lowercase().contains(needle)
}

fn names<'a>(doc: &'a Value, sec: &str) -> impl Iterator<Item = &'a String> {
    doc.get(sec)
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|m| m.keys())
}

fn action_hits(s: Store, doc: &Value, needle: &str, out: &mut Vec<Hit>) {
    let rotate = "rotate secret of ";
    if !needle.is_empty() {
        let found: Vec<(u8, usize, String)> = names(doc, ClientEntry::section())
            .filter_map(|c| {
                let label = format!("{rotate}{}…", c.to_lowercase());
                let r = word_at(&label, needle)?;
                let r = if r >= rotate.len() {
                    rank(&label[rotate.len()..], needle).unwrap_or(3)
                } else {
                    2
                };
                Some((r, c.len(), c.clone()))
            })
            .filter(|(_, _, c)| s.can_write_item(&[ClientEntry::section().into(), c.clone()]))
            .collect();
        for c in best(found, PER_SECTION) {
            out.push(Hit {
                section: "Actions",
                icon: "refresh",
                label: format!("Rotate secret of {c}…"),
                facet: ClientEntry::ENTITY.into(),
                go: Go::To(
                    Route::Detail(ClientEntry::section().into(), c),
                    Some(("tab", "secrets")),
                ),
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
                go: Go::To(Route::List(sec.into()), Some(("new", "1"))),
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
                go: Go::To(Route::Review, None),
            });
        }
    }
}

fn entity_hits(doc: &Value, status: Option<&Status>, needle: &str, out: &mut Vec<Hit>) {
    for sec in crate::route::SECTIONS {
        let (title, icon) = (
            crate::screens::plural(sec),
            crate::screens::entity_icon(sec),
        );
        let found: Vec<(u8, usize, &String)> = names(doc, sec)
            .filter_map(|n| rank(&n.to_lowercase(), needle).map(|r| (r, n.len(), n)))
            .collect();
        let total = found.len();
        for n in best(found, PER_SECTION) {
            let v = doc.get(sec).and_then(|m| m.get(n)).unwrap_or(&Value::Null);
            out.push(Hit {
                section: title,
                icon,
                label: n.clone(),
                facet: entity_facet(sec, n, v, status),
                go: Go::To(Route::Detail(sec.into(), n.clone()), None),
            });
        }
        if total > PER_SECTION {
            out.push(Hit {
                section: title,
                icon: "search",
                label: format!(
                    "Show all {total} {} matching {needle}",
                    title.to_lowercase()
                ),
                facet: String::new(),
                go: Go::Find(Route::List(sec.into()), needle.to_string()),
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
    if needle.is_empty() {
        return;
    }
    let Some(schema) = s.schema.get_untracked() else {
        return;
    };
    let skip = burrow_admin_logic::tree::entity_sections(&schema);
    let skipped: Vec<&str> = skip.iter().map(String::as_str).collect();
    let cards = crate::form::settings_cards(&schema.root, &skipped);
    let found: Vec<(u8, usize, (String, String))> = burrow_admin_logic::tree::fields(&schema.root)
        .iter()
        .filter(|f| !skip.contains(&f.key))
        .filter_map(|f| {
            let label = cards
                .iter()
                .find(|c| c.object && c.at.is_empty() && c.fields.iter().any(|g| g.key == f.key))
                .map_or_else(|| f.node.label.clone(), |c| c.title.clone());
            let r = [&label, &f.node.label, &f.key]
                .into_iter()
                .filter_map(|t| rank(&t.to_lowercase(), needle))
                .min()?;
            Some((r, label.len(), (label, f.key.clone())))
        })
        .collect();
    for (label, key) in best(found, PER_SECTION) {
        let tab = burrow_admin_logic::settings_tabs::tab_of(&key).key();
        out.push(Hit {
            section: "Settings",
            icon: "settings",
            label,
            facet: key,
            go: Go::To(Route::Settings, Some(("tab", tab))),
        });
    }
}

fn history_hits(s: Store, needle: &str, out: &mut Vec<Hit>) {
    if needle.is_empty() {
        return;
    }
    let found: Vec<(String, String, String, u64)> = s.history.with_untracked(|h| {
        h.iter()
            .flatten()
            .filter_map(|e| {
                let summary = e
                    .changes
                    .first()
                    .map(|c| c.summary.as_str())
                    .unwrap_or_default();
                hit(needle, summary)
                    .then(|| (e.rev.clone(), summary.to_string(), e.id.to_string(), e.at))
            })
            .take(PER_SECTION)
            .collect()
    });
    for (rev, summary, id, at) in found {
        out.push(Hit {
            section: "Activity",
            icon: "history",
            label: format!("{} · {summary}", s.version(&rev)),
            facet: crate::format::relative(at),
            go: Go::To(Route::Activity(Some(id)), None),
        });
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
        match h.go {
            Go::Find(to, text) => route::go_search(to, &text),
            Go::To(to, query) => {
                let new = query
                    .is_some_and(|(k, _)| k == "new")
                    .then(|| to.section().to_string());
                route::go_then(s, to, query, move || {
                    if let Some(sec) = new.clone() {
                        s.creating.set(Some(sec));
                    }
                });
            }
        }
    };
    let last = Memo::new(move |_| hits.with(Vec::len).saturating_sub(1));
    let current = Memo::new(move |_| active.get().min(last.get()));
    Effect::new(move |_| {
        let id = option_id(current.get());
        if hits.with(Vec::is_empty) {
            return;
        }
        if let Some(el) = document().get_element_by_id(&id) {
            let opts = web_sys::ScrollIntoViewOptions::new();
            opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
            el.scroll_into_view_with_scroll_into_view_options(&opts);
        }
    });
    let body = move || {
        let list = hits.get();
        let needle = q.get_untracked().trim().to_lowercase();
        if list.is_empty() {
            return view! { <li class="px-2.5 py-3 text-sm text-text-muted" role="presentation" data-testid="search-none">"Nothing matches"</li> }.into_any();
        }
        let mut last = "";
        list.into_iter()
            .enumerate()
            .map(|(i, h)| {
                let head = (h.section != last).then(|| view! { <PickHead text=h.section/> });
                last = h.section;
                let label = marked(&h.label, &needle);
                let (icon, note) = (h.icon, h.facet.clone());
                view! {
                    {head}
                    <PickOption
                        icon=icon
                        id=option_id(i)
                        active=Signal::derive(move || current.get() == i)
                        on_hover=Callback::new(move |_| active.set(i))
                        on_pick=Callback::new(move |_| run(h.clone()))
                        note=note
                    >
                        {label}
                    </PickOption>
                }
            })
            .collect_view()
            .into_any()
    };
    let keys = Callback::new(move |e: web_sys::KeyboardEvent| {
        let n = hits.with(Vec::len);
        let page = 8;
        let to = match e.key().as_str() {
            "ArrowDown" => Some((current.get_untracked() + 1).min(n.saturating_sub(1))),
            "ArrowUp" => Some(current.get_untracked().saturating_sub(1)),
            "PageDown" => Some((current.get_untracked() + page).min(n.saturating_sub(1))),
            "PageUp" => Some(current.get_untracked().saturating_sub(page)),
            "Home" if e.ctrl_key() => Some(0),
            "End" if e.ctrl_key() => Some(n.saturating_sub(1)),
            "Enter" => {
                e.prevent_default();
                if let Some(h) = hits.with(|l| l.get(current.get_untracked()).cloned()) {
                    run(h);
                }
                None
            }
            _ => None,
        };
        if let Some(to) = to {
            e.prevent_default();
            active.set(to);
        }
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
                        aria-autocomplete="list"
                        aria-controls=LIST_ID
                        aria-activedescendant=move || (!hits.with(Vec::is_empty)).then(|| option_id(current.get()))
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
                <ul id=LIST_ID class="max-h-[60vh] overflow-y-auto p-1.5" role="listbox" aria-label="Results">{body}</ul>
                <div class="flex items-center gap-3 border-t border-border px-4 py-2.5 text-xs text-text-muted">
                    <span>"↑↓ move"</span>
                    <span>"Enter run"</span>
                    <span class="max-sm:hidden">"Ctrl+Home/End first/last"</span>
                    <span class="ml-auto">"Searches services, clients, visitors, settings and history"</span>
                </div>
        </Modal>
    }
}

fn option_id(i: usize) -> String {
    format!("search-opt-{i}")
}

#[cfg(test)]
mod tests {
    use super::{best, rank};

    #[test]
    fn an_exact_name_ranks_before_a_prefix_a_word_and_a_substring() {
        assert_eq!(rank("svc0042", "svc0042"), Some(0));
        assert_eq!(rank("svc00421", "svc0042"), Some(1));
        assert_eq!(rank("old-svc0042", "svc0042"), Some(2));
        assert_eq!(rank("oldsvc0042", "svc0042"), Some(3));
        assert_eq!(rank("web", "svc"), None);
    }

    #[test]
    fn only_the_best_few_are_kept_in_rank_then_length_then_name_order() {
        let found = vec![
            (3, 4, "xweb"),
            (1, 5, "web-a"),
            (0, 3, "web"),
            (1, 4, "webb"),
            (2, 5, "a-web"),
        ];
        assert_eq!(best(found.clone(), 3), ["web", "webb", "web-a"]);
        assert_eq!(best(found, 9).len(), 5);
    }
}
