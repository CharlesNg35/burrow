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

const MORE: &str = "cursor-pointer rounded-xs font-semibold text-accent-ink outline-none underline-offset-2 hover:underline focus-visible:ring-3 focus-visible:ring-accent-weak";

pub fn next_step(shown: usize, total: usize, step: usize) -> usize {
    total.min(shown.saturating_add(step.max(1)))
}

pub fn shown_of(total: usize, shown: usize) -> usize {
    total.min(shown)
}

#[component]
pub fn ShowMore(
    shown: RwSignal<usize>,
    #[prop(into)] total: Signal<usize>,
    step: usize,
    #[prop(into)] noun: String,
    #[prop(optional, into)] class: String,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    let rest = Memo::new(move |_| total.get().saturating_sub(shown.get()));
    let noun = StoredValue::new(noun);
    view! {
        <Show when=move || { rest.get() > 0 }>
            <div
                class=format!("flex flex-wrap items-center gap-x-3 gap-y-1 px-4 py-2 text-[12.5px] text-text-muted {class}")
                data-testid=(!testid.is_empty()).then_some(testid)
                data-show-more=""
            >
                <span aria-live="polite">{move || noun.with_value(|n| format!("{} of {} {n} shown", shown.get().min(total.get()), total.get()))}</span>
                <button
                    type="button"
                    class=MORE
                    data-testid="show-more"
                    on:click=move |_| shown.set(next_step(shown.get_untracked(), total.get_untracked(), step))
                >
                    {move || format!("Show {} more", rest.get().min(step))}
                </button>
                <Show when=move || { rest.get() > step }>
                    <button type="button" class=MORE data-testid="show-all" on:click=move |_| shown.set(total.get_untracked())>
                        {move || show_all(total.get())}
                    </button>
                </Show>
            </div>
        </Show>
    }
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

    #[test]
    fn each_step_reveals_more_and_stops_at_the_end() {
        assert_eq!(next_step(200, 1000, 200), 400);
        assert_eq!(next_step(900, 1000, 200), 1000);
        assert_eq!(next_step(0, 5, 0), 1);
        assert_eq!(shown_of(3, 200), 3);
        assert_eq!(shown_of(5000, 200), 200);
    }
}
