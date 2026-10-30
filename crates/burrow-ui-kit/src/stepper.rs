use leptos::prelude::*;

use crate::icon::Icon;
use crate::input::{INPUT_H, TextInput};

pub fn stepped(text: &str, default: i64, by: i64, min: i64, max: i64) -> String {
    let now = text.trim().parse::<i64>().unwrap_or(default);
    now.saturating_add(by).clamp(min, max).to_string()
}

#[component]
pub fn Stepper(
    #[prop(into)] id: String,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_input: Callback<String>,
    default: i64,
    min: i64,
    max: i64,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] invalid: Signal<bool>,
    #[prop(into)] label: String,
    #[prop(optional)] touch_only: bool,
) -> impl IntoView {
    let only = if touch_only { " sm:hidden" } else { "" };
    let step = move |by: i64| {
        let next = stepped(&value.get_untracked(), default, by, min, max);
        on_input.run(next);
    };
    let button = format!(
        "flex {INPUT_H} w-[38px] shrink-0 cursor-pointer items-center justify-center rounded-input border border-border-strong bg-surface text-text-muted hover:bg-surface-3 hover:text-text disabled:cursor-not-allowed disabled:opacity-55{only}"
    );
    let at_min =
        Signal::derive(move || value.get().trim().parse::<i64>().unwrap_or(default) <= min);
    let at_max =
        Signal::derive(move || value.get().trim().parse::<i64>().unwrap_or(default) >= max);
    view! {
        <div class="flex items-center gap-1" data-stepper="">
            <button type="button" class=button.clone() aria-label=format!("{label} down") data-testid="step-down" disabled=move || at_min.get() on:click=move |_| step(-1)>
                <Icon name="minus" small=true/>
            </button>
            <div class=if touch_only { "w-[88px] sm:w-[96px]" } else { "w-[88px]" }>
                <TextInput id=id value=value on_input=on_input numeric=true invalid=invalid placeholder=placeholder label=label.clone()/>
            </div>
            <button type="button" class=button aria-label=format!("{label} up") data-testid="step-up" disabled=move || at_max.get() on:click=move |_| step(1)>
                <Icon name="plus" small=true/>
            </button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_starts_from_the_default_and_stays_in_range() {
        assert_eq!(stepped("", 1, 1, 1, 1000), "2");
        assert_eq!(stepped("", 1, -1, 1, 1000), "1");
        assert_eq!(stepped("1000", 1, 1, 1, 1000), "1000");
        assert_eq!(stepped(" 7 ", 1, -1, 1, 1000), "6");
        assert_eq!(stepped("abc", 1, 1, 1, 1000), "2");
    }
}
