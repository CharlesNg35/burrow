use std::collections::BTreeMap;

use burrow_admin_logic::goto::Kind;
use burrow_admin_logic::wording::count_of;
use burrow_admin_proto::diag::Severity;
use burrow_ui_kit::{Deferred, Icon, Kbd};
use leptos::prelude::*;

use super::goto_list::goto_list;
use crate::edit::TextEdit;

pub const PLACEHOLDER: &str = "Go to file, service, client…";
const TEXT_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Clone, Copy)]
pub struct GoTo {
    pub open: RwSignal<bool>,
    pub(super) q: RwSignal<String>,
    pub(super) active: RwSignal<usize>,
    pub(super) shown: RwSignal<BTreeMap<Kind, usize>>,
    pub(super) text_q: RwSignal<String>,
    typed: StoredValue<u64>,
}

impl GoTo {
    pub fn new() -> GoTo {
        GoTo {
            open: RwSignal::new(false),
            q: RwSignal::new(String::new()),
            active: RwSignal::new(0),
            shown: RwSignal::new(BTreeMap::new()),
            text_q: RwSignal::new(String::new()),
            typed: StoredValue::new(0),
        }
    }

    pub(super) fn typed(&self, q: String) {
        self.q.set(q.clone());
        self.typed.update_value(|n| *n += 1);
        let n = self.typed.get_value();
        let me = *self;
        set_timeout(
            move || {
                if me.typed.try_get_value() == Some(n) {
                    me.text_q.set(q);
                }
            },
            TEXT_AFTER,
        );
    }

    fn show(&self) {
        self.q.set(String::new());
        self.text_q.set(String::new());
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
    if mac { "Cmd+P" } else { "Ctrl+P" }
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

pub fn panel(g: GoTo, t: TextEdit) -> impl IntoView {
    g.open.set(false);
    hotkey(g);
    crate::shown::popover(g.open, "file-goto", "file-goto-panel");
    move || {
        g.open.get().then(|| view! {
            <div id="file-goto-panel" class="absolute top-full right-0 left-0 z-40 mt-1 overflow-hidden rounded-dialog border border-border-strong bg-surface shadow-overlay sm:left-auto sm:w-[560px] dark:bg-surface-2" data-testid="file-goto-panel">
                <Deferred load=move || goto_list(g, t) what="Go to"/>
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
