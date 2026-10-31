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

pub const MAX_TOASTS: usize = 3;

pub fn push_toast(list: &mut Vec<Toast>, toast: Toast) {
    list.push(toast);
    let excess = list.len().saturating_sub(MAX_TOASTS);
    list.drain(..excess);
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
    let place = move || {
        let bottom = match crate::bar::bar_top() {
            Some(t) => (crate::bar::view_height() - t + 8.0).max(16.0),
            None => 16.0,
        };
        if let Some(el) = host.get_untracked() {
            let el: &web_sys::HtmlElement = &el;
            let _ = el.style().set_property("bottom", &format!("{bottom}px"));
        }
    };
    Effect::new(move |_| {
        toasts.track();
        tick.track();
        place();
    });
    let resize = window_event_listener(leptos::ev::resize, move |_| place());
    let scroll = window_event_listener(leptos::ev::scroll, move |_| place());
    on_cleanup(move || {
        resize.remove();
        scroll.remove();
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
                    .take(MAX_TOASTS)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u64) -> Toast {
        Toast {
            id,
            tone: Tone::Info,
            text: String::new(),
            action: None,
        }
    }

    #[test]
    fn a_fourth_toast_drops_the_oldest() {
        let mut list = Vec::new();
        for id in 1..=4 {
            push_toast(&mut list, t(id));
        }
        assert_eq!(list.iter().map(|x| x.id).collect::<Vec<_>>(), [2, 3, 4]);
    }
}
