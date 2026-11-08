use burrow_ui_kit::{Button, ButtonLook, Dialog, TextInput, Tone};
use leptos::prelude::*;

use crate::edit::{TextEdit, new_name};

const HINT: &str = "Holds services, clients and visitors";

pub(super) fn name_dialog(t: TextEdit) -> impl IntoView {
    let stem = RwSignal::new(String::new());
    let touched = RwSignal::new(false);
    let folder = Memo::new(move |_| {
        t.text()
            .map(|x| crate::edit::folder_of(&x))
            .unwrap_or_else(|| "server.d".into())
    });
    let checked = Memo::new(move |_| {
        let typed = stem.get();
        let new = t.new_files.get();
        t.text()
            .map(|x| new_name(&x, &new, &typed))
            .unwrap_or_else(|| Err("The files are not loaded yet".into()))
    });
    let problem = Signal::derive(move || {
        checked.with(|c| c.as_ref().err().cloned().filter(|_| touched.get()))
    });
    Effect::new(move |_| {
        if t.adding.get() {
            stem.set(String::new());
            touched.set(false);
        }
    });
    let add = move || {
        touched.set(true);
        if let Ok(name) = checked.get_untracked() {
            t.add_file(name);
        }
    };
    view! {
        <Dialog open=t.adding title="New drop-in file" sub=HINT icon="plus" testid="file-new-dialog">
            <form class="flex flex-col gap-3" on:submit=move |ev| { ev.prevent_default(); add(); }>
                <TextInput
                    id="file-new-name"
                    label="File name"
                    value=stem
                    on_input=Callback::new(move |v: String| { stem.set(v); touched.set(true); })
                    prefix=format!("{}/", folder.get_untracked())
                    suffix=".yaml"
                    mono=true
                    invalid=Signal::derive(move || problem.with(Option::is_some))
                    described_by="file-new-problem"
                />
                {move || problem.get().map(|p| view! { <p id="file-new-problem" class="text-xs text-danger [overflow-wrap:anywhere]" data-testid="file-new-problem">{p}</p> })}
                <div class="flex flex-wrap justify-end gap-2">
                    <Button label="Cancel" testid="file-new-cancel" on_click=move |_| t.adding.set(false)/>
                    <Button label="Add" variant=ButtonLook::Primary testid="file-new-add" disabled=Signal::derive(move || checked.with(Result::is_err)) on_click=move |_| add()/>
                </div>
            </form>
        </Dialog>
    }
}

pub(super) fn close_dialog(t: TextEdit) -> impl IntoView {
    let open = RwSignal::new(false);
    Effect::new(move |_| open.set(t.closing.with(Option::is_some)));
    Effect::new(move |_| {
        if !open.get() && t.closing.with_untracked(Option::is_some) {
            t.closing.set(None);
        }
    });
    let title = Signal::derive(move || format!("Close {}?", t.closing.get().unwrap_or_default()));
    view! {
        <Dialog open=open title=title sub="It is a new file; its text is lost." icon="alert" tone=Tone::Warning testid="file-close-dialog">
            <div class="flex flex-wrap justify-end gap-2">
                <Button label="Keep it" testid="file-close-keep" on_click=move |_| t.closing.set(None)/>
                <Button label="Close the file" variant=ButtonLook::Danger testid="file-close-confirm" on_click=move |_| {
                    if let Some(name) = t.closing.get_untracked() {
                        t.drop_new(&name);
                    }
                    t.closing.set(None);
                }/>
            </div>
        </Dialog>
    }
}
