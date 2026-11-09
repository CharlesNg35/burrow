use burrow_ui_kit::{Button, ButtonLook, Dialog, Tone};
use leptos::prelude::*;

use crate::edit::TextEdit;

pub(super) fn leave_dialog(t: TextEdit) -> impl IntoView {
    let open = RwSignal::new(false);
    Effect::new(move |_| {
        let now = t.leaving.with(|l| !l.is_empty());
        open.set(now);
        if now {
            set_timeout(focus_stay, std::time::Duration::from_millis(50));
        }
    });
    Effect::new(move |_| {
        if !open.get() && t.leaving.with_untracked(|l| !l.is_empty()) {
            t.stay();
        }
    });
    let title = Signal::derive(move || t.leaving.with(|l| crate::edit::leave_title(l)));
    let sub = Signal::derive(move || t.leaving.with(|l| crate::edit::leave_question(l)));
    view! {
        <Dialog open=open title=title sub=sub icon="alert" tone=Tone::Warning testid="file-leave-dialog" dismissable=false>
            <div class="flex flex-wrap justify-end gap-2">
                <Button label="Stay" variant=ButtonLook::Primary testid="file-leave-stay" on_click=move |_| t.stay()/>
                <Button label="Leave and lose it" variant=ButtonLook::Danger testid="file-leave-confirm" on_click=move |_| t.leave()/>
            </div>
        </Dialog>
    }
}

fn focus_stay() {
    if let Some(el) = document()
        .query_selector("[data-testid=file-leave-stay]")
        .ok()
        .flatten()
        && let Ok(el) = wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(el)
    {
        let _ = el.focus();
    }
}
