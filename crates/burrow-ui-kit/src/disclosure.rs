use leptos::prelude::*;

use crate::icon::Icon;

#[component]
pub fn Disclosure(
    #[prop(into)] id: String,
    #[prop(into)] summary: Signal<String>,
    open: RwSignal<bool>,
    #[prop(optional, into)] on_toggle: Option<Callback<bool>>,
    #[prop(optional)] table: bool,
    #[prop(optional, into)] grid: String,
    children: Children,
) -> impl IntoView {
    let region = format!("{id}-region");
    let (outer, button) = if table {
        ("", "py-3")
    } else {
        ("md:col-span-2", "border-t border-rowline pt-3")
    };
    view! {
        <div class=outer data-disclosure="" data-open=move || open.get().to_string()>
            <button
                type="button"
                id=id.clone()
                class=format!("flex w-full cursor-pointer items-center gap-1.5 rounded-xs text-left text-[12.5px] font-semibold text-text-muted outline-none hover:text-text focus-visible:ring-3 focus-visible:ring-accent-weak {button}")
                aria-expanded=move || open.get().to_string()
                aria-controls=region.clone()
                data-testid="more-settings"
                on:click=move |_| {
                    let now = !open.get_untracked();
                    open.set(now);
                    if let Some(cb) = on_toggle {
                        cb.run(now);
                    }
                }
            >
                <span class=move || if open.get() { "inline-flex rotate-90 transition-transform" } else { "inline-flex transition-transform" } aria-hidden="true">
                    <Icon name="chevron-right" small=true/>
                </span>
                <span>{move || summary.get()}</span>
            </button>
            <div
                id=region.clone()
                role="region"
                aria-labelledby=id
                hidden=move || (!open.get()).then_some("until-found")
                on:beforematch=move |_: leptos::ev::Event| open.set(true)
                data-testid="more-region"
            >
                <div class=grid>{children()}</div>
            </div>
        </div>
    }
}
