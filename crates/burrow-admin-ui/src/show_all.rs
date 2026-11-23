use burrow_admin_logic::top::{SHOW_FEWER, more, show_all};
use burrow_ui_kit::{LINK, Tone, chip_class};
use leptos::prelude::*;

pub(crate) use burrow_admin_logic::top::kept;

fn toggle(
    open: RwSignal<bool>,
    closed: impl Fn() -> String + Send + Sync + 'static,
    class: String,
    testid: &'static str,
) -> impl IntoView {
    let label = move || {
        if open.get() {
            SHOW_FEWER.to_string()
        } else {
            closed()
        }
    };
    view! {
        <button type="button" class=format!("cursor-pointer font-semibold {class}") aria-expanded=move || open.get().to_string() data-testid=testid on:click=move |_| open.update(|o| *o = !*o)>{label}</button>
    }
}

#[component]
pub(crate) fn ShowAll(
    open: RwSignal<bool>,
    #[prop(into)] total: Signal<usize>,
    limit: usize,
) -> impl IntoView {
    move || {
        (total.get() > limit).then(|| {
            view! {
                <p class="border-t border-border px-4 py-2.5 text-right text-[13px]">
                    {toggle(open, move || show_all(total.get()), LINK.to_string(), "show-all")}
                </p>
            }
        })
    }
}

pub(crate) fn more_chip(open: RwSignal<bool>, hidden: usize) -> impl IntoView {
    toggle(
        open,
        move || more(hidden),
        chip_class(Tone::Neutral, false),
        "links-more",
    )
}
