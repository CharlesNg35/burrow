use burrow_ui_kit::{Button, Icon, StickyBar};
use leptos::prelude::*;

#[derive(Clone)]
pub struct SaveBar {
    pub problems: Vec<String>,
    pub count: usize,
    pub changed: Signal<usize>,
    pub blocker: Signal<Option<&'static str>>,
    pub on_discard: Callback<()>,
    pub on_save: Callback<()>,
}

pub fn save_bar(b: SaveBar) -> AnyView {
    let SaveBar {
        problems,
        count,
        changed,
        blocker,
        on_discard,
        on_save,
    } = b;
    let bad = count > 0;
    let status = if bad {
        let fields = (!problems.is_empty()).then(|| {
            view! { <span class="truncate font-normal text-text-muted">{format!(" · {}", problems.join(", "))}</span> }
        });
        view! { <span class="flex min-w-0 items-center gap-1.5 text-sm font-semibold text-danger"><Icon name="alert-circle"/>{burrow_admin_logic::wording::count_of(count, "problem")}{fields}</span> }.into_any()
    } else {
        let text = move || match (blocker.get(), changed.get()) {
            (Some(b), _) => b.to_string(),
            (None, 0) => "No changes yet".to_string(),
            (None, n) => format!(
                "{} changed",
                burrow_admin_logic::wording::count_of(n, "field")
            ),
        };
        view! { <span class="text-sm text-text-muted" data-testid="fields-changed">{text}</span> }
            .into_any()
    };
    let reason = move || {
        if bad {
            "Fix the problems first".to_string()
        } else if let Some(b) = blocker.get() {
            b.to_string()
        } else if changed.get() == 0 {
            "Nothing changed yet".to_string()
        } else {
            String::new()
        }
    };
    view! {
        <StickyBar danger=bad testid="save-bar">
            {status}
            <span class="ml-auto"></span>
            <Button label=Signal::derive(move || if changed.get() == 0 { "Close".to_string() } else { "Discard".to_string() }) variant="ghost" testid="discard-edit" on_click=move |_| on_discard.run(())/>
            <Button
                label="Add to changes"
                variant="primary"
                testid="save"
                disabled=Signal::derive(move || bad || changed.get() == 0 || blocker.get().is_some())
                reason=Signal::derive(reason)
                on_click=move |_| on_save.run(())
            />
        </StickyBar>
    }
    .into_any()
}
