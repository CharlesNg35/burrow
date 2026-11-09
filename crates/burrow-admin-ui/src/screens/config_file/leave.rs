use burrow_ui_kit::{Button, ButtonLook, Dialog, Tone};
use leptos::prelude::*;

use crate::edit::TextEdit;

pub(super) fn leave_dialog(t: TextEdit) -> impl IntoView {
    let open = RwSignal::new(false);
    Effect::new(move |_| open.set(t.leaving.with(Option::is_some)));
    Effect::new(move |_| {
        if !open.get() && t.leaving.with_untracked(Option::is_some) {
            t.stay();
        }
    });
    let title = Signal::derive(move || {
        t.leaving
            .with(|l| l.as_ref().map(|u| u.title()).unwrap_or_default())
    });
    let sub = Signal::derive(move || {
        t.leaving
            .with(|l| l.as_ref().map(|u| u.question()).unwrap_or_default())
    });
    view! {
        <Dialog open=open title=title sub=sub icon="alert" tone=Tone::Warning testid="file-leave-dialog" dismissable=false>
            <div class="flex flex-wrap justify-end gap-2">
                <Button label="Stay" variant=ButtonLook::Primary testid="file-leave-stay" on_click=move |_| t.stay()/>
                <Button label="Leave and lose it" variant=ButtonLook::Danger testid="file-leave-confirm" on_click=move |_| t.leave()/>
            </div>
        </Dialog>
    }
}
