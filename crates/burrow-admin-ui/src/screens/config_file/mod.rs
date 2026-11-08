//! The Config file tab: the server's config as text, and the raw text editor for write admins.
//! `ConfigFile` is the entry; edit.rs holds the edit-mode chrome, editor.rs the editor itself (behind one function, loaded lazily).

mod edit;
mod edit_as;
mod editor;
mod follow;
mod goto;
mod goto_list;
mod new_file;
mod problems;

use burrow_admin_logic::running::{difference_note, differences};
use burrow_admin_proto::msg;
use burrow_admin_proto::serde_json;
use burrow_admin_proto::view::{ConfigView, Shown, ShownRequest};
use burrow_ui_kit::{Callout, CodeBlock, Toggle, Tone};
use leptos::prelude::*;

use crate::api;
use crate::route::{Link, Route};
use crate::store::store;

type Loaded = Option<Result<ConfigView, String>>;

fn fetch(show: Shown, into: RwSignal<Loaded>, latest: StoredValue<u64>) {
    latest.update_value(|n| *n += 1);
    let asked = latest.get_value();
    api::request(
        msg::req::CONFIG_VIEW,
        api::body(&ShownRequest { show }),
        move |r| {
            if latest.try_get_value() != Some(asked) {
                return;
            }
            let got = r
                .map_err(|e| e.message)
                .and_then(|v| serde_json::from_value::<ConfigView>(v).map_err(|e| e.to_string()));
            let _ = into.try_set(Some(got));
        },
    );
}

fn differs() -> Memo<Option<String>> {
    let s = store();
    Memo::new(move |_| {
        s.state.with(|st| {
            st.as_ref()
                .and_then(|st| difference_note(s.status.with(|ss| differences(st, ss.as_deref()))))
        })
    })
}

fn differs_view(n: String) -> impl IntoView {
    view! {
        <Callout tone=Tone::Warning title=n testid="file-differs">
            <Link to=Route::Overview class="font-semibold text-accent-ink no-underline hover:underline" testid="file-differs-link">"See Needs attention"</Link>
        </Callout>
    }
}

fn running_note(note: Memo<Option<String>>) -> impl IntoView {
    move || {
        note.get()
            .map(|n| view! { <div class="mb-3">{differs_view(n)}</div> })
    }
}

fn edit_notes(note: Memo<Option<String>>) -> impl IntoView {
    let t = store().text;
    move || {
        let differ = note.get();
        let read_only = edit_as::read_only_text(t);
        let waiting = edit_as::waits_text(t);
        let empty = t.empty_new_files();
        if differ.is_none() && read_only.is_none() && waiting.is_none() && empty.is_empty() {
            return None;
        }
        Some(view! {
            <div class="mb-3 flex min-w-0 flex-col gap-2" data-testid="file-notes">
                {differ.map(differs_view)}
                {read_only.map(|r| edit_as::note_line("lock", r, "file-read-only", false))}
                {waiting.map(|n| edit_as::note_line("alert", n, "file-edit-as-waits", true))}
                {empty.into_iter().map(|n| edit_as::note_line("info", crate::edit::not_created(&n), "file-not-created", false)).collect_view()}
            </div>
        })
    }
}

fn file_body(loaded: RwSignal<Loaded>) -> impl IntoView {
    let text = Signal::derive(move || {
        loaded.with(|l| match l {
            Some(Ok(v)) => v.text(),
            _ => String::new(),
        })
    });
    let shape = Memo::new(move |_| {
        loaded.with(|l| match l {
            None => None,
            Some(Ok(v)) => Some(Ok(v.download_name())),
            Some(Err(e)) => Some(Err(e.clone())),
        })
    });
    move || match shape.get() {
        None => view! { <p class="text-sm text-text-muted">"Loading…"</p> }.into_any(),
        Some(Err(e)) => {
            view! { <p class="text-sm text-danger" data-testid="file-error">{e}</p> }.into_any()
        }
        Some(Ok(name)) => {
            view! { <CodeBlock text=text file_name=name testid="file-text"/> }.into_any()
        }
    }
}

#[component]
pub fn ConfigFile() -> impl IntoView {
    let s = store();
    let t = s.text;
    let offered = Memo::new(move |_| {
        s.welcome
            .with(|w| w.as_ref().is_some_and(|w| w.offers(msg::req::CONFIG_VIEW)))
    });
    let can_edit = Memo::new(move |_| {
        s.can_write()
            && s.welcome
                .with(|w| w.as_ref().is_some_and(|w| w.offers(msg::req::CONFIG_TEXT)))
    });
    let editing = Memo::new(move |_| can_edit.get() && t.open.get());
    let show = RwSignal::new(Shown::Written);
    let loaded: RwSignal<Loaded> = RwSignal::new(None);
    let rev = Memo::new(move |_| s.state.with(|st| st.as_ref().map(|st| st.rev.clone())));
    let latest = StoredValue::new(0u64);
    Effect::new(move |_| {
        rev.track();
        let now = show.get();
        if offered.get() {
            fetch(now, loaded, latest);
        }
    });
    Effect::new(move |_| {
        rev.track();
        if editing.get_untracked() {
            t.fetch();
        }
    });
    crate::store::on_reconnect(move || {
        if offered.get_untracked() && loaded.with_untracked(|l| matches!(l, Some(Err(_)))) {
            fetch(show.get_untracked(), loaded, latest);
        }
        if editing.get_untracked() {
            t.fetch();
        }
    });
    let waiting = RwSignal::new(false);
    let asking = RwSignal::new(false);
    let go = goto::GoTo::new();
    let toggle = move || {
        offered.get().then(|| {
            view! {
            <Toggle
                id="file-defaults"
                checked=Signal::derive(move || show.get() == Shown::WithDefaults)
                on_change=Callback::new(move |on: bool| {
                    show.set(if on { Shown::WithDefaults } else { Shown::Written })
                })
                label=Signal::stored("Show defaults too".to_string())
            />
                }
        })
    };
    let head = move || {
        let (sub, right) = if editing.get() {
            (None, edit::edit_buttons(s, asking, go))
        } else {
            let edit_button = can_edit.get().then(|| edit::edit_button(s, waiting));
            let right =
                view! { <span data-testid="file-toggle">{toggle}</span>{edit_button} }.into_any();
            (subline(can_edit.get(), s.can_write()), right)
        };
        burrow_ui_kit::card_head(
            "The config the server reads".into(),
            sub.map(str::to_string),
            right,
        )
    };
    let body = move || {
        if editing.get() {
            edit::edit_body(s).into_any()
        } else if offered.get() {
            view! { {edit::waiting_note(s, waiting)} {file_body(loaded)} }.into_any()
        } else {
            view! { <p class="text-sm text-text-muted" data-testid="file-older">"This server runs an older burrow that cannot show its config file here. Update burrow on the server."</p> }.into_any()
        }
    };
    let shell = move || {
        if editing.get() {
            "mb-4"
        } else {
            "mb-4 rounded-card border border-border bg-surface"
        }
    };
    let head_box = move || {
        if editing.get() {
            "relative rounded-t-card border border-b-0 border-border bg-surface"
        } else {
            ""
        }
    };
    let note = differs();
    let content = move || {
        if editing.get() {
            view! {
                <div class="min-w-0 pt-3" data-testid="file-body">
                    {edit_notes(note)}
                    {body}
                </div>
            }
            .into_any()
        } else {
            view! {
                <div class="min-w-0 px-4 py-3" data-testid="file-body">
                    {running_note(note)}
                    {body}
                </div>
            }
            .into_any()
        }
    };
    view! {
        <section class=shell data-card="file" data-testid="config-file" data-editing=move || editing.get().to_string() data-rev=move || loaded.with(|l| l.as_ref().and_then(|r| r.as_ref().ok()).map(|v| v.rev.clone()).unwrap_or_default()) data-state-rev=move || rev.get().unwrap_or_default()>
            <div class=head_box data-testid="file-head">{head}{move || editing.get().then(|| goto::panel(go, t))}</div>
            {content}
            {edit::cancel_dialog(s, asking)}
            {new_file::name_dialog(s.text)}
            {new_file::close_dialog(s.text)}
        </section>
    }
}

const VIEW_ONLY: &str = "View only";
const EDIT_ELSEWHERE: &str = "View only; edit in other tabs or with burrow edit";

fn subline(can_edit: bool, can_write: bool) -> Option<&'static str> {
    match (can_edit, can_write) {
        (true, _) => None,
        (false, true) => Some(EDIT_ELSEWHERE),
        (false, false) => Some(VIEW_ONLY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subline_speaks_only_when_the_text_cannot_be_edited_here() {
        assert_eq!(subline(true, true), None);
        assert_eq!(
            subline(false, true),
            Some(EDIT_ELSEWHERE),
            "an older server"
        );
        assert_eq!(subline(false, false), Some(VIEW_ONLY), "a read admin");
    }
}
