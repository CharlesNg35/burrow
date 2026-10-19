use leptos::prelude::*;

use crate::basic::button_class;
use crate::icon::Icon;

#[component]
pub fn Drawer(
    #[prop(into)] open: RwSignal<bool>,
    #[prop(into)] title: Signal<String>,
    #[prop(optional)] testid: &'static str,
    children: ChildrenFn,
) -> impl IntoView {
    view! {
        <Show when=move || open.get()>
            <div class="fixed inset-0 z-40 bg-black/40" on:click=move |_| open.set(false)></div>
            <aside
                class="fixed inset-y-0 right-0 z-50 flex w-full max-w-3xl flex-col bg-surface shadow-overlay"
                role="dialog"
                aria-modal="true"
                aria-label=move || title.get()
                data-testid=(!testid.is_empty()).then_some(testid)
                on:keydown=move |e| {
                    if e.key() == "Escape" {
                        open.set(false);
                    }
                }
            >
                <div class="flex items-center justify-between border-b border-border px-4 py-3">
                    <h2 class="text-lg font-semibold">{move || title.get()}</h2>
                    <button
                        type="button"
                        class=button_class("ghost", true)
                        aria-label="Close"
                        on:click=move |_| open.set(false)
                    >
                        <Icon name="close"/>
                    </button>
                </div>
                <div class="flex-1 overflow-y-auto p-4">{children()}</div>
            </aside>
        </Show>
    }
}

#[component]
pub fn Dialog(
    #[prop(into)] open: RwSignal<bool>,
    #[prop(into)] title: Signal<String>,
    #[prop(optional)] testid: &'static str,
    #[prop(default = true)] dismissable: bool,
    children: ChildrenFn,
) -> impl IntoView {
    view! {
        <Show when=move || open.get()>
            <div class="fixed inset-0 z-40 bg-black/40"></div>
            <div
                class="fixed top-1/2 left-1/2 z-50 w-[min(36rem,calc(100vw-2rem))] -translate-1/2 rounded-card bg-surface p-5 shadow-overlay"
                role="dialog"
                aria-modal="true"
                aria-label=move || title.get()
                data-testid=(!testid.is_empty()).then_some(testid)
                on:keydown=move |e| {
                    if dismissable && e.key() == "Escape" {
                        open.set(false);
                    }
                }
            >
                <h2 class="mb-3 text-lg font-semibold">{move || title.get()}</h2>
                <div>{children()}</div>
            </div>
        </Show>
    }
}
