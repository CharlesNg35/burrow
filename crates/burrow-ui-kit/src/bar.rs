use leptos::prelude::*;

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
