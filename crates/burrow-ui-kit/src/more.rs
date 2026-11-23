use leptos::prelude::*;

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
                        {move || format!("Show all {}", total.get())}
                    </button>
                </Show>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::{next_step, shown_of};

    #[test]
    fn each_step_reveals_more_and_stops_at_the_end() {
        assert_eq!(next_step(200, 1000, 200), 400);
        assert_eq!(next_step(900, 1000, 200), 1000);
        assert_eq!(next_step(0, 5, 0), 1);
        assert_eq!(shown_of(3, 200), 3);
        assert_eq!(shown_of(5000, 200), 200);
    }
}
