use std::collections::BTreeMap;

use burrow_admin_logic::goto::{Found, Item, Kind, Sources, find};
use burrow_admin_logic::wording::count_of;
use burrow_admin_proto::diag::Severity;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{Icon, Kbd, PickHead, PickOption, marked};
use leptos::prelude::*;

use crate::edit::{Spot, TextEdit};
use crate::store::{Store, store};

pub const FIRST: usize = 8;
const MORE: usize = 50;
const LIST: &str = "file-goto-list";
const PLACEHOLDER: &str = "Go to file, service, client…";

#[derive(Clone, Copy)]
pub struct GoTo {
    pub open: RwSignal<bool>,
    q: RwSignal<String>,
    active: RwSignal<usize>,
    shown: RwSignal<BTreeMap<Kind, usize>>,
}

#[derive(Clone, PartialEq)]
enum Row {
    Head(Kind),
    Hit(Kind, Found),
    More(Kind, usize),
}

impl GoTo {
    pub fn new() -> GoTo {
        GoTo {
            open: RwSignal::new(false),
            q: RwSignal::new(String::new()),
            active: RwSignal::new(0),
            shown: RwSignal::new(BTreeMap::new()),
        }
    }

    fn show(&self) {
        self.q.set(String::new());
        self.active.set(0);
        self.shown.set(BTreeMap::new());
        self.open.set(true);
    }
}

pub fn shortcut() -> &'static str {
    let mac = window()
        .navigator()
        .user_agent()
        .is_ok_and(|a| a.contains("Mac OS") || a.contains("iPhone") || a.contains("iPad"));
    if mac { "⌘P" } else { "Ctrl+P" }
}

pub fn problem_files(t: TextEdit) -> Vec<String> {
    let main = t
        .text()
        .and_then(|x| x.files.first().map(|f| f.name.clone()));
    let open = t.tabs.get();
    let mut out: Vec<String> = Vec::new();
    t.problems.with(|ps| {
        for d in ps
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error | Severity::Warning))
        {
            let Some(file) = d.file.clone().or_else(|| main.clone()) else {
                continue;
            };
            if !open.contains(&file) && !out.contains(&file) {
                out.push(file);
            }
        }
    });
    out
}

pub fn problems_label(n: usize) -> String {
    match n {
        1 => "1 file has problems".into(),
        n => format!("{} have problems", count_of(n, "file")),
    }
}

fn items(s: Store, main: &str) -> Vec<Item> {
    let mut out: Vec<Item> = s
        .with_files(|i| {
            i.files
                .iter()
                .flat_map(|f| {
                    f.items.iter().map(|p| Item {
                        path: p.clone(),
                        file: f.name.clone(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let doc = s.edit.doc();
    for section in [
        ServiceEntry::section(),
        ClientEntry::section(),
        VisitorEntry::section(),
    ] {
        for name in doc
            .get(section)
            .and_then(|v| v.as_object())
            .into_iter()
            .flat_map(|m| m.keys())
        {
            let path = vec![section.to_string(), name.clone()];
            if !out.iter().any(|i| i.path == path) {
                out.push(Item {
                    path,
                    file: main.to_string(),
                });
            }
        }
    }
    out
}

fn rows(g: GoTo, t: TextEdit) -> Vec<Row> {
    let s = store();
    let mut files: Vec<String> = t
        .text()
        .map(|x| x.files.iter().map(|f| f.name.clone()).collect())
        .unwrap_or_default();
    files.extend(t.new_files.get());
    let main = files.first().cloned().unwrap_or_default();
    let items = items(s, &main);
    let recent = t.recent.get();
    let problems = problem_files(t);
    let src = Sources {
        files: &files,
        items: &items,
        recent: &recent,
        problems: &problems,
    };
    let shown = g.shown.get();
    let mut out = Vec::new();
    for (kind, hits) in find(&g.q.get(), &src) {
        let limit = shown.get(&kind).copied().unwrap_or(FIRST);
        out.push(Row::Head(kind));
        let total = hits.len();
        out.extend(hits.into_iter().take(limit).map(|h| Row::Hit(kind, h)));
        if total > limit {
            out.push(Row::More(kind, total - limit));
        }
    }
    out
}

fn pick(g: GoTo, t: TextEdit, row: Row) {
    match row {
        Row::Head(_) => {}
        Row::More(kind, _) => g.shown.update(|m| {
            let n = m.entry(kind).or_insert(FIRST);
            *n += MORE;
        }),
        Row::Hit(_, f) => {
            g.open.set(false);
            match (f.path, f.line) {
                (Some(p), _) => t.open_at(&f.file, Spot::Path(p)),
                (None, Some(l)) => t.open_at(&f.file, Spot::Line(l)),
                (None, None) => t.open_file(&f.file),
            }
        }
    }
}

pub fn button(g: GoTo, t: TextEdit) -> impl IntoView {
    let count = Memo::new(move |_| problem_files(t).len());
    let label = move || {
        let n = count.get();
        if n == 0 {
            PLACEHOLDER.to_string()
        } else {
            format!("{PLACEHOLDER} {}", problems_label(n))
        }
    };
    view! {
        <button
            type="button"
            id="file-goto"
            class="flex h-7 min-w-0 cursor-pointer items-center gap-2 rounded-input border border-border bg-input px-2 text-xs text-text-faint outline-none hover:border-border-strong focus-visible:border-accent focus-visible:ring-3 focus-visible:ring-accent-weak sm:w-[260px]"
            aria-label=label
            aria-haspopup="listbox"
            aria-expanded=move || g.open.get().to_string()
            title=label
            data-testid="file-goto"
            on:click=move |_| if g.open.get_untracked() { g.open.set(false) } else { g.show() }
        >
            <Icon name="search" small=true/>
            <span class="truncate max-sm:hidden">{PLACEHOLDER}</span>
            {move || (count.get() > 0).then(|| view! {
                <span class="rounded-full bg-warning-weak px-1.5 text-2xs font-semibold leading-4 text-warning" data-testid="file-goto-problems">{count.get()}</span>
            })}
            <span class="ml-auto shrink-0 max-sm:hidden"><Kbd text=shortcut()/></span>
        </button>
    }
}

fn keys(g: GoTo, t: TextEdit, list: Memo<Vec<Row>>) -> impl Fn(web_sys::KeyboardEvent) + Copy {
    move |e: web_sys::KeyboardEvent| {
        let picks: Vec<usize> = list.with(|l| {
            l.iter()
                .enumerate()
                .filter(|(_, r)| !matches!(r, Row::Head(_)))
                .map(|(i, _)| i)
                .collect()
        });
        let at = picks.iter().position(|i| *i == g.active.get_untracked());
        match e.key().as_str() {
            "ArrowDown" => {
                e.prevent_default();
                let next = at.map_or(0, |a| (a + 1).min(picks.len().saturating_sub(1)));
                if let Some(i) = picks.get(next) {
                    g.active.set(*i);
                }
            }
            "ArrowUp" => {
                e.prevent_default();
                if let Some(i) = picks.get(at.map_or(0, |a| a.saturating_sub(1))) {
                    g.active.set(*i);
                }
            }
            "Enter" => {
                e.prevent_default();
                let row = list.with(|l| l.get(g.active.get_untracked()).cloned());
                if let Some(row) = row {
                    pick(g, t, row);
                }
            }
            _ => {}
        }
    }
}

fn first_pick(list: &[Row]) -> usize {
    list.iter()
        .position(|r| !matches!(r, Row::Head(_)))
        .unwrap_or(0)
}

fn option(g: GoTo, t: TextEdit, i: usize, row: Row, needle: &str) -> AnyView {
    let active = Signal::derive(move || g.active.get() == i);
    let hover = Callback::new(move |_| g.active.set(i));
    let id = format!("file-goto-opt-{i}");
    match row.clone() {
        Row::Head(kind) => view! { <PickHead text=kind.title()/> }.into_any(),
        Row::More(_, n) => view! {
            <PickOption icon="more" id=id active=active on_hover=hover on_pick=Callback::new(move |_| pick(g, t, row.clone())) testid="file-goto-more">
                {format!("… {n} more")}
            </PickOption>
        }
        .into_any(),
        Row::Hit(kind, f) => {
            let label = marked(&f.label, needle);
            view! {
                <PickOption icon=kind.icon() id=id active=active on_hover=hover on_pick=Callback::new(move |_| pick(g, t, row.clone())) note=f.note.clone() testid="file-goto-hit">
                    {label}
                </PickOption>
            }
            .into_any()
        }
    }
}

pub fn panel(g: GoTo, t: TextEdit) -> impl IntoView {
    g.open.set(false);
    let list = Memo::new(move |_| if g.open.get() { rows(g, t) } else { Vec::new() });
    Effect::new(move |_| {
        g.q.track();
        g.active.set(list.with_untracked(|l| first_pick(l)));
    });
    hotkey(g);
    crate::shown::popover(g.open, "file-goto", "file-goto-panel");
    let on_key = keys(g, t, list);
    let body = move || {
        let needle = g.q.get().trim().to_lowercase();
        let rows = list.get();
        if rows.is_empty() {
            return view! { <li class="px-2.5 py-3 text-sm text-text-muted" role="presentation">"Nothing matches"</li> }.into_any();
        }
        rows.into_iter()
            .enumerate()
            .map(|(i, r)| option(g, t, i, r, &needle))
            .collect_view()
            .into_any()
    };
    move || {
        g.open.get().then(|| view! {
            <div id="file-goto-panel" class="absolute top-full right-0 left-0 z-40 mt-1 overflow-hidden rounded-dialog border border-border-strong bg-surface shadow-overlay sm:left-auto sm:w-[560px] dark:bg-surface-2" data-testid="file-goto-panel" on:keydown=on_key>
                <div class="flex items-center gap-2.5 border-b border-border px-3">
                    <Icon name="search"/>
                    <input
                        id="file-goto-input"
                        type="text"
                        role="combobox"
                        aria-expanded="true"
                        aria-controls=LIST
                        aria-autocomplete="list"
                        aria-activedescendant=move || format!("file-goto-opt-{}", g.active.get())
                        aria-label="Go to a file, service, client or visitor"
                        placeholder=PLACEHOLDER
                        autocomplete="off"
                        class="h-11 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-text-faint"
                        prop:value=move || g.q.get()
                        on:input:target=move |e| g.q.set(e.target().value())
                    />
                    <span class="max-sm:hidden"><Kbd text="esc"/></span>
                </div>
                <ul id=LIST class="max-h-[min(60vh,420px)] overflow-y-auto p-1.5" role="listbox" aria-label="Go to">{body}</ul>
            </div>
        })
    }
}

fn hotkey(g: GoTo) {
    let key = window_event_listener(leptos::ev::keydown, move |e| {
        let p = e.key().eq_ignore_ascii_case("p");
        if p && (e.ctrl_key() || e.meta_key()) && !e.alt_key() && !e.shift_key() {
            e.prevent_default();
            if g.open.get_untracked() {
                g.open.set(false);
            } else {
                g.show();
            }
        }
    });
    on_cleanup(move || key.remove());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_go_to_button_counts_files_with_problems() {
        assert_eq!(problems_label(1), "1 file has problems");
        assert_eq!(problems_label(2), "2 files have problems");
    }
}
