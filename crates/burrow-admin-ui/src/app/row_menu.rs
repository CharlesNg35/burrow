use burrow_ui_kit::{Icon, MENU_ITEM, button_class, menu_class};
use leptos::prelude::*;

const MENU: &str = "row-menu";
const BUTTON: &str = "row-menu-button";

#[derive(Clone)]
pub(crate) struct Item {
    pub label: &'static str,
    pub testid: &'static str,
    pub danger: bool,
    pub run: Callback<()>,
}

pub(crate) fn row_menus() -> RwSignal<Option<String>> {
    let open: RwSignal<Option<String>> = RwSignal::new(None);
    let shown = super::shown_while(open, Option::is_some, |o| *o = None);
    super::popover(shown, BUTTON, MENU);
    open
}

pub(crate) fn row_menu(
    open: RwSignal<Option<String>>,
    key: String,
    label: String,
    items: Vec<Item>,
) -> AnyView {
    let mine = {
        let key = key.clone();
        Memo::new(move |_| open.with(|m| m.as_deref() == Some(key.as_str())))
    };
    let items = items
        .into_iter()
        .map(|it| {
            let class = if it.danger {
                format!("{MENU_ITEM} text-danger")
            } else {
                MENU_ITEM.to_string()
            };
            view! { <button type="button" class=class data-testid=it.testid on:click=move |_| it.run.run(())>{it.label}</button> }
        })
        .collect_view();
    view! {
        <div class="relative">
            <button
                type="button"
                class=format!("{} w-9 px-0", button_class("ghost", false))
                aria-label=label.clone()
                title=label
                aria-haspopup="true"
                aria-expanded=move || mine.get().to_string()
                aria-controls=move || mine.get().then_some(MENU)
                id=move || mine.get().then_some(BUTTON)
                data-testid="row-more"
                on:click=move |_| open.update(|m| *m = if m.as_deref() == Some(key.as_str()) { None } else { Some(key.clone()) })
            >
                <Icon name="more"/>
            </button>
            <div class=menu_class("top-10 right-0 w-56", false) id=move || mine.get().then_some(MENU) tabindex="-1" data-testid="row-menu" class:hidden=move || !mine.get() on:click=move |_| open.set(None)>
                {items}
            </div>
        </div>
    }
    .into_any()
}
