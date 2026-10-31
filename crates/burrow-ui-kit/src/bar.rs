use leptos::prelude::*;

pub fn view_height() -> f64 {
    web_sys::window()
        .and_then(|w| w.inner_height().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}

pub fn bar_top() -> Option<f64> {
    let h = view_height();
    let all = web_sys::window()?
        .document()?
        .query_selector_all("[data-sticky-bar]")
        .ok()?;
    (0..all.length())
        .filter_map(|i| all.item(i))
        .filter_map(|n| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(n).ok())
        .map(|e| e.get_bounding_client_rect())
        .filter(|r| r.height() > 0.0)
        .map(|r| r.top())
        .reduce(f64::min)
        .filter(|t| *t > 0.0 && *t < h)
}

#[component]
pub fn StickyBar(
    #[prop(optional, into)] danger: Signal<bool>,
    #[prop(optional)] testid: &'static str,
    #[prop(optional)] class: &'static str,
    #[prop(optional)] flush: bool,
    children: Children,
) -> impl IntoView {
    let gap = if flush { "" } else { "mt-6" };
    view! {
        <div
            class=move || format!("sticky bottom-4 z-20 {gap} flex flex-wrap items-center gap-3 rounded-card border bg-surface px-4 py-3 shadow-overlay max-sm:[&>*:first-child]:basis-full {class} {}", if danger.get() { "border-danger" } else { "border-border" })
            data-testid=(!testid.is_empty()).then_some(testid)
            data-sticky-bar=""
        >
            {children()}
        </div>
    }
}

pub fn bar_status(text: Signal<String>) -> AnyView {
    view! {
        <span class="flex min-w-0 items-center gap-1.5 text-sm text-text-muted" data-testid="bar-status">
            <crate::Icon name="info" small=true/>
            <span>{move || text.get()}</span>
        </span>
    }
    .into_any()
}
