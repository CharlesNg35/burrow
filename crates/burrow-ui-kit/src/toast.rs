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
    let tick = RwSignal::new(0u32);
    if let Ok(h) = set_interval_with_handle(
        move || {
            if toasts.with_untracked(|t| !t.is_empty()) {
                tick.update(|n| *n = n.wrapping_add(1));
            }
        },
        std::time::Duration::from_millis(250),
    ) {
        on_cleanup(move || h.clear());
    }
    let host: NodeRef<leptos::html::Div> = NodeRef::new();
    Effect::new(move |_| {
        toasts.track();
        tick.track();
        let Some(w) = web_sys::window() else {
            return;
        };
        let h = w
            .inner_height()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let top = w
            .document()
            .and_then(|d| d.query_selector_all("[data-sticky-bar]").ok())
            .and_then(|all| {
                (0..all.length())
                    .filter_map(|i| all.item(i))
                    .filter_map(|n| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(n).ok())
                    .map(|e| e.get_bounding_client_rect())
                    .filter(|r| r.height() > 0.0)
                    .map(|r| r.top())
                    .reduce(f64::min)
            });
        let bottom = match top {
            Some(t) if t > 0.0 && t < h => (h - t + 8.0).max(16.0),
            _ => 16.0,
        };
        if let Some(el) = host.get_untracked() {
            let el: &web_sys::HtmlElement = &el;
            let _ = el.style().set_property("bottom", &format!("{bottom}px"));
        }
    });
    view! {
        <div
            class="pointer-events-none fixed right-4 bottom-4 z-50 flex w-[min(380px,calc(100vw-32px))] flex-col gap-2"
            node_ref=host
            aria-live="polite"
            data-testid="toasts"
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
                            <div
                                class="pointer-events-auto flex items-center gap-2.5 rounded-[10px] border border-border-strong bg-surface px-3.5 py-2.5 text-[13px] shadow-overlay dark:bg-surface-2"
                                role="status"
                                data-testid="toast"
                                on:mouseenter=move |_| on_hover.run(true)
                                on:mouseleave=move |_| on_hover.run(false)
                            >
                                <span class=t.tone.text()><Icon name=t.tone.icon()/></span>
                                <span class="flex-1">{t.text.clone()}</span>
                                {t.action.clone().map(|a| view! {
                                    <button type="button" class="cursor-pointer text-sm font-semibold text-accent-ink hover:underline" on:click=move |_| on_action.run(id)>{a}</button>
                                })}
                            </div>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
