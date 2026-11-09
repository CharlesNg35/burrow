use burrow_ui_kit::{Button, ButtonLook, ButtonSize, Callout, Deferred, Dialog, Tone};
use leptos::prelude::*;

use crate::route::{Link, Route};
use crate::store::Store;

use super::editor::{EditorProps, text_editor};

pub(super) fn edit_button(s: Store, waiting: RwSignal<bool>) -> AnyView {
    view! {
        <Button label="Edit" icon="edit" size=ButtonSize::Small testid="file-edit" on_click=move |_| {
            if untrack(|| s.edit.dirty()) {
                waiting.set(true);
            } else {
                waiting.set(false);
                s.text.start();
            }
        }/>
    }
    .into_any()
}

pub(super) fn waiting_title(pending: usize) -> String {
    match pending {
        0 => "You have edits in other tabs not added to the changes yet".into(),
        1 => "1 change is waiting in Review".into(),
        n => format!("{n} changes are waiting in Review"),
    }
}

pub(super) fn waiting_note(s: Store, waiting: RwSignal<bool>) -> impl IntoView {
    move || {
        if !waiting.get() || !s.edit.dirty() {
            return None;
        }
        Some(view! {
            <div class="mb-3">
                <Callout tone=Tone::Info title=waiting_title(s.edit.pending()) testid="file-waiting">
                    <p class="mb-1.5">"Apply or discard them first, then edit the text."</p>
                    <Link to=Route::Review class="font-semibold text-accent-ink no-underline hover:underline" testid="file-waiting-link">"Open Review"</Link>
                </Callout>
            </div>
        })
    }
}

fn review(s: Store) {
    let t = s.text;
    let why = t.held_now();
    if let Some(why) = why.first() {
        s.toast(Tone::Warning, why.reason(), None);
        return;
    }
    if t.ops().is_empty() {
        if let Some(name) = untrack(|| t.empty_new_files()).first() {
            s.toast(Tone::Info, crate::edit::not_created(name), None);
        }
        return;
    }
    if untrack(|| t.dirty()) && t.unsavable.with_untracked(Option::is_none) {
        s.edit.open_text(s);
    }
}

pub(super) fn edit_buttons(s: Store, asking: RwSignal<bool>, go: super::goto::GoTo) -> AnyView {
    let t = s.text;
    let blocked = Signal::derive(move || !t.dirty() || t.unsavable.with(Option::is_some));
    let reason = Signal::derive(move || match t.unsavable.get() {
        Some(r) => r,
        None if t.dirty() => String::new(),
        None => "No edits yet".into(),
    });
    view! {
        {super::goto::button(go, t)}
        {super::edit_as::controls(t)}
        <Button label="Cancel" size=ButtonSize::Small testid="file-cancel" on_click=move |_| {
            if untrack(|| t.dirty()) {
                asking.set(true);
            } else {
                t.close();
            }
        }/>
        <Button label="Review changes" variant=ButtonLook::Primary size=ButtonSize::Small testid="file-review" disabled=blocked reason=reason on_click=move |_| review(s)/>
    }
    .into_any()
}

pub(super) fn cancel_dialog(s: Store, asking: RwSignal<bool>) -> impl IntoView {
    let t = s.text;
    view! {
        <Dialog open=asking title="Discard your edits?" sub="The files stay as the server has them." icon="alert" tone=Tone::Warning testid="file-cancel-dialog">
            <div class="flex flex-wrap justify-end gap-2">
                <Button label="Keep editing" testid="file-keep" on_click=move |_| asking.set(false)/>
                <Button label="Discard edits" variant=ButtonLook::Danger testid="file-discard" on_click=move |_| {
                    asking.set(false);
                    t.close();
                }/>
            </div>
        </Dialog>
    }
}

pub(super) fn edit_body(s: Store) -> impl IntoView {
    let t = s.text;
    let hash = Memo::new(move |_| {
        t.loaded.with(|l| match l {
            None => None,
            Some(Ok(x)) => Some(Ok(x.schema_hash.clone())),
            Some(Err(e)) => Some(Err(e.clone())),
        })
    });
    move || {
        match hash.get() {
        None => view! { <p class="text-sm text-text-muted" data-testid="file-loading">"Loading…"</p> }.into_any(),
        Some(Err(e)) => view! {
            <div class="text-sm" data-testid="file-error">
                <p class="mb-2 text-danger">{e}</p>
                <Button label="Try again" size=ButtonSize::Small icon="refresh" on_click=move |_| t.fetch()/>
            </div>
        }
        .into_any(),
        Some(Ok(_)) => {
            let props = EditorProps {
                text: Signal::derive(move || t.text()),
                values: t.values.into(),
                problems: t.problems.into(),
                on_change: Callback::new(move |(name, text): (String, String)| t.set_value(&name, text)),
                on_review: Callback::new(move |_| review(s)),
                before_review: t.before_review,
                edit_as: t.edit_as,
                active: t.active,
                on_add: Callback::new(move |_| t.adding.set(true)),
                on_close: Callback::new(move |(name, dirty): (String, bool)| t.close_tab(&name, dirty)),
                on_dirty: Callback::new(move |(name, dirty): (String, bool)| t.set_unsaved(&name, dirty)),
                tabs: t.tabs.into(),
                spot: t.spot,
                edit: t,
            };
            view! { <Deferred load=move || text_editor(props) what="the editor" testid="editor-load"/> }.into_any()
        }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_waiting_note_counts_the_changes_in_review() {
        assert_eq!(waiting_title(1), "1 change is waiting in Review");
        assert_eq!(waiting_title(3), "3 changes are waiting in Review");
        assert!(waiting_title(0).contains("not added"));
    }
}
