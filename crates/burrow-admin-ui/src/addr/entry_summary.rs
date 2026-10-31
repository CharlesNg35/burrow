use burrow_admin_logic::web::target_value_shown;
use burrow_admin_logic::wording::{MAIN_TARGET, NOT_SET};
use burrow_ui_kit::{ComboOption, Icon, StatusDot, Tone};
use leptos::prelude::*;

use super::entries::Place;
use super::entry_target::main_shown;
use crate::form::plain;

fn rest(main: bool, weight: Option<&str>) -> String {
    let mut tail = String::new();
    if main {
        tail.push_str(&format!(" ({MAIN_TARGET})"));
    }
    if let Some(w) = weight {
        tail.push_str(&format!(" · weight {w}"));
    }
    tail
}

pub(super) fn line(target: &str, main: bool, weight: Option<&str>) -> String {
    format!("→ {target}{}", rest(main, weight))
}

pub(super) fn target_line(target: String, main: bool, weight: Option<String>) -> AnyView {
    let tail = rest(main, weight.as_deref());
    let whole = line(&target, main, weight.as_deref());
    view! { <span class="text-text-muted" title=whole data-entry-to="">"→ "<span class="font-mono">{target}</span>{tail}</span> }.into_any()
}

pub(super) fn summary(
    place: &Place,
    i: usize,
    options: Signal<Vec<ComboOption>>,
    balance: Signal<bool>,
    expanded: RwSignal<Option<usize>>,
    menu: AnyView,
) -> impl IntoView {
    let main = main_shown(place);
    let current = {
        let place = place.clone();
        Memo::new(move |_| place.read().get(i).cloned().unwrap_or_default())
    };
    let name = Signal::derive(move || current.with(|r| r.name.clone()));
    let tone = Signal::derive(move || {
        let n = name.get();
        options
            .with(|o| o.iter().find(|x| x.value == n).and_then(|x| x.dot))
            .unwrap_or(Tone::Neutral)
    });
    let text = move || {
        current.with(|r| {
            let own = r.target.as_ref().map(target_value_shown);
            let weight = balance
                .get()
                .then(|| r.weight.as_ref().map_or_else(|| "1".to_string(), plain));
            let main_now = own.is_none();
            let to = own
                .or_else(|| main.get())
                .unwrap_or_else(|| NOT_SET.to_string());
            target_line(to, main_now, weight)
        })
    };
    let open = Memo::new(move |_| expanded.get() == Some(i));
    let toggle = move |_| expanded.update(|x| *x = if *x == Some(i) { None } else { Some(i) });
    view! {
        <div class="order-1 flex items-center gap-1 py-1 @min-[560px]:hidden" data-entry-summary="">
            <button type="button" class="flex min-w-0 flex-1 cursor-pointer flex-col items-start gap-0.5 py-1 text-left" aria-expanded=move || open.get().to_string() data-testid="entry-toggle" on:click=toggle>
                <span class="text-sm font-semibold text-text"><StatusDot on=false text=name tone=tone/></span>
                <span class="max-w-full text-xs [overflow-wrap:anywhere]" data-entry-sum="">{text}</span>
            </button>
            {menu}
            <button type="button" class="flex size-9 shrink-0 cursor-pointer items-center justify-center rounded-md text-text-muted hover:bg-surface-3" aria-label=move || if open.get() { "Close entry" } else { "Open entry" } on:click=toggle>
                <span class=move || if open.get() { "flex rotate-180" } else { "flex" }><Icon name="chevron-down" small=true/></span>
            </button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_summary_names_where_the_entry_goes_and_its_weight() {
        assert_eq!(
            line("127.0.0.1:8080", true, Some("1")),
            "→ 127.0.0.1:8080 (main target) · weight 1"
        );
        assert_eq!(line("10.0.0.13:8080", false, None), "→ 10.0.0.13:8080");
        assert_eq!(line("not set", true, None), "→ not set (main target)");
    }
}
