use leptos::prelude::*;

const ITEM: &str = "relative min-w-0 pl-[0.85em] before:absolute before:left-0 before:w-[0.85em] before:text-center before:content-['·']";

pub fn inline(items: Vec<AnyView>) -> AnyView {
    view! {
        <span class="block overflow-hidden">
            <span class="-ml-[0.85em] flex flex-wrap items-baseline">
                {items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| view! {
                        <span class=ITEM data-item="">
                            {(i > 0).then(|| view! { <span class="sr-only">", "</span> })}
                            {item}
                        </span>
                    })
                    .collect_view()}
            </span>
        </span>
    }
    .into_any()
}

#[component]
pub fn Inline(#[prop(into)] items: Signal<Vec<String>>) -> impl IntoView {
    move || inline(items.get().into_iter().map(IntoAny::into_any).collect())
}
