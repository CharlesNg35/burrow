use burrow_admin_logic::web::target_value_shown;
use burrow_admin_logic::wording::{MAIN_TARGET, NOT_SET};
use burrow_ui_kit::{ComboOption, Icon, StatusDot, Tone};
use leptos::prelude::*;

use super::entries::Place;
use super::entry_target::main_shown;
use crate::form::plain;

pub(super) fn line(target: Option<&str>, own: bool, weight: Option<&str>) -> String {
    let target = target.unwrap_or(NOT_SET);
    let to = if own {
        format!("→ {target}")
    } else {
        format!("→ {target} ({MAIN_TARGET})")
    };
    match weight {
        Some(w) => format!("{to} · weight {w}"),
        None => to,
    }
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
            line(
                own.as_deref().or(main.get().as_deref()),
                own.is_some(),
                weight.as_deref(),
            )
        })
    };
    let open = Memo::new(move |_| expanded.get() == Some(i));
    let toggle = move |_| expanded.update(|x| *x = if *x == Some(i) { None } else { Some(i) });
    view! {
        <div class="flex items-center gap-1 py-1 sm:hidden" data-entry-summary="">
            <button type="button" class="flex min-w-0 flex-1 cursor-pointer flex-col items-start gap-0.5 py-1 text-left" aria-expanded=move || open.get().to_string() data-testid="entry-toggle" on:click=toggle>
                <span class="text-sm font-semibold text-text"><StatusDot on=false text=name tone=tone/></span>
                <span class="max-w-full text-xs text-text-muted [overflow-wrap:anywhere]" data-entry-sum="">{text}</span>
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
            line(Some("127.0.0.1:8080"), false, Some("1")),
            "→ 127.0.0.1:8080 (main target) · weight 1"
        );
        assert_eq!(line(Some("10.0.0.13:8080"), true, None), "→ 10.0.0.13:8080");
        assert_eq!(line(None, false, None), "→ not set (main target)");
    }
}
