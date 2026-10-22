use leptos::prelude::*;

use crate::basic::Tone;
use crate::icon::Icon;

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub tone: Tone,
    pub text: String,
    pub action: Option<String>,
}

#[component]
pub fn ToastHost(
    #[prop(into)] toasts: Signal<Vec<Toast>>,
    #[prop(into)] on_action: Callback<u64>,
    #[prop(into)] on_hover: Callback<bool>,
) -> impl IntoView {
    view! {
        <div
            class="pointer-events-none fixed right-4 bottom-4 z-50 flex w-[min(380px,calc(100vw-32px))] flex-col gap-2 [body:has([data-sticky-bar])_&]:bottom-24"
            aria-live="polite"
            data-testid="toasts"
            on:mouseenter=move |_| on_hover.run(true)
            on:mouseleave=move |_| on_hover.run(false)
        >
            {move || {
                toasts
                    .get()
                    .into_iter()
                    .rev()
                    .take(3)
                    .map(|t| {
                        let id = t.id;
                        view! {
                            <div class="flex items-center gap-2.5 rounded-[10px] border border-border-strong bg-surface px-3.5 py-2.5 text-[13px] shadow-overlay dark:bg-surface-2" role="status" data-testid="toast">
                                <span class=t.tone.text()><Icon name=t.tone.icon()/></span>
                                <span class="flex-1">{t.text.clone()}</span>
                                {t.action.clone().map(|a| view! {
                                    <button type="button" class="pointer-events-auto cursor-pointer text-sm font-semibold text-accent-ink hover:underline" on:click=move |_| on_action.run(id)>{a}</button>
                                })}
                            </div>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
