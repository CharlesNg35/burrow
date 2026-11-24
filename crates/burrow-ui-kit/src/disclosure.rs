use leptos::prelude::*;

use crate::icon::Icon;

#[component]
pub fn Disclosure(
    #[prop(into)] id: String,
    #[prop(into)] summary: Signal<String>,
    open: RwSignal<bool>,
    #[prop(optional, into)] on_toggle: Option<Callback<bool>>,
    #[prop(optional)] table: bool,
    #[prop(optional)] flush: bool,
    #[prop(optional, into)] grid: String,
    #[prop(optional, into)] title: String,
    #[prop(optional, into)] detail: Signal<String>,
    #[prop(optional)] aside: Option<ViewFn>,
    #[prop(optional)] testid: Option<&'static str>,
    children: Children,
) -> impl IntoView {
    let region = format!("{id}-region");
    let titled = !title.is_empty();
    let (outer, button) = match (table, flush) {
        (true, _) => ("", "py-3"),
        (false, true) => ("md:col-span-2", ""),
        (false, false) => ("md:col-span-2", "border-t border-rowline pt-3"),
    };
    let width = if aside.is_some() { "" } else { "w-full" };
    let head = view! {
        <button
            type="button"
            id=id.clone()
            class=format!("flex {width} cursor-pointer items-center gap-1.5 rounded-xs text-left text-[12.5px] font-semibold text-text-muted outline-none hover:text-text focus-visible:ring-3 focus-visible:ring-accent-weak {button}")
            aria-expanded=move || open.get().to_string()
            aria-controls=region.clone()
            data-testid=testid.unwrap_or("more-settings")
            on:click=move |_| {
                let now = !open.get_untracked();
                open.set(now);
                if let Some(cb) = on_toggle {
                    cb.run(now);
                }
            }
        >
            {(!titled).then(|| view! {
                <span class="inline-flex" aria-hidden="true">
                    {move || view! { <Icon name=if open.get() { "chevron-down" } else { "chevron-right" } small=true/> }}
                </span>
            })}
            {titled.then(|| view! { <span class="text-[13.5px] font-bold text-text">{title.clone()}</span> })}
            <span class:ml-auto=titled>{move || summary.get()}</span>
            {move || {
                let d = detail.get();
                (!d.is_empty()).then(|| view! { <span class="ml-1.5 font-normal" data-detail="">{d}</span> })
            }}
            {titled.then(|| view! {
                <span class="inline-flex" aria-hidden="true">
                    {move || view! { <Icon name=if open.get() { "chevron-down" } else { "chevron-right" } small=true/> }}
                </span>
            })}
        </button>
    };
    let head = match aside {
        Some(a) => view! {
            <div class="flex flex-wrap items-center gap-x-1.5 gap-y-1 text-[12.5px] text-text-muted">{head}{a.run()}</div>
        }
        .into_any(),
        None => head.into_any(),
    };
    view! {
        <div class=outer data-disclosure="" data-open=move || open.get().to_string()>
            {head}
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
