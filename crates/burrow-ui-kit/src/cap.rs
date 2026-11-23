use leptos::prelude::*;

use crate::basic::{LINK, Tone, chip_class};

pub const LINKS: usize = 12;
pub const SHOW_FEWER: &str = "Show fewer";

pub fn show_all(total: usize) -> String {
    format!("Show all {total}")
}

pub fn more(n: usize) -> String {
    format!("+{n} more")
}

pub fn kept<T>(mut all: Vec<T>, open: bool, n: usize) -> Vec<T> {
    if !open {
        all.truncate(n);
    }
    all
}

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
pub fn ShowAll(
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

#[component]
pub fn MoreChip(open: RwSignal<bool>, hidden: usize) -> impl IntoView {
    toggle(
        open,
        move || more(hidden),
        chip_class(Tone::Neutral, false),
        "links-more",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_list_keeps_the_first_rows_and_an_open_one_keeps_all() {
        let all: Vec<u32> = (0..20).collect();
        assert_eq!(kept(all.clone(), false, 8), (0..8).collect::<Vec<_>>());
        assert_eq!(kept(all.clone(), true, 8), all);
        assert_eq!(kept(vec![1, 2], false, 8), [1, 2]);
        assert_eq!(show_all(300), "Show all 300");
        assert_eq!(more(188), "+188 more");
    }
}
