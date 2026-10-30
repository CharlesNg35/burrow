mod dialog;
mod site;

use burrow_admin_logic::logins::{config, logins, passes, summary, used_by};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::value::child;
use burrow_config::{Entry, entity_path};
use burrow_ui_kit::{Button, Dialog, FieldRow, Icon, MENU_ITEM, Tone, menu_class};
use leptos::prelude::*;

use crate::route::{Link, Route};
use crate::store::store;

pub(crate) use site::site_editor;

const MENU: &str = "login-menu";
const MENU_BUTTON: &str = "login-menu-button";

#[derive(Clone, PartialEq)]
pub(crate) enum Open {
    Closed,
    Add,
    Edit(String),
}

fn base() -> Path {
    entity_path(&Entity::Login)
}

fn site_links(sites: Vec<String>) -> AnyView {
    if sites.is_empty() {
        return view! { <span class="text-xs text-text-muted" data-testid="login-used">"Not used by any site yet"</span> }.into_any();
    }
    let n = sites.len();
    let links = sites
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            view! {
                {(i > 0).then_some(", ")}
                <Link to=Route::Detail(burrow_config::ServiceEntry::section().into(), s.clone()) class="font-semibold text-accent-ink no-underline hover:underline">{s}</Link>
            }
        })
        .collect_view();
    view! {
        <span class="text-xs text-text-muted" data-testid="login-used">
            {format!("Used by {n} site{}: ", if n == 1 { "" } else { "s" })}
            {links}
        </span>
    }
    .into_any()
}

fn card_menu(
    name: String,
    menu: RwSignal<Option<String>>,
    open: RwSignal<Open>,
    renaming: RwSignal<Option<String>>,
    removing: RwSignal<Option<String>>,
) -> impl IntoView {
    let shown = {
        let name = name.clone();
        Memo::new(move |_| menu.with(|m| m.as_deref() == Some(name.as_str())))
    };
    let (n1, n2, n3, n4) = (name.clone(), name.clone(), name.clone(), name.clone());
    view! {
        <div class="relative ml-auto">
            <button
                type="button"
                class="flex size-7 cursor-pointer items-center justify-center rounded-md text-text-muted hover:bg-surface-3"
                aria-label=format!("More for {name}")
                aria-haspopup="true"
                aria-expanded=move || shown.get().to_string()
                aria-controls=move || shown.get().then_some(MENU)
                id=move || shown.get().then_some(MENU_BUTTON)
                data-testid="login-more"
                on:click=move |_| menu.update(|m| *m = if m.as_deref() == Some(n1.as_str()) { None } else { Some(n1.clone()) })
            >
                <Icon name="more"/>
            </button>
            <Show when=move || shown.get()>
                <div class=menu_class("top-8 right-0 w-40", false) id=MENU tabindex="-1" data-testid="login-menu" on:click=move |_| menu.set(None)>
                    <button type="button" class=MENU_ITEM data-testid="login-edit" on:click={ let n = n2.clone(); move |_| open.set(Open::Edit(n.clone())) }>"Edit…"</button>
                    <button type="button" class=MENU_ITEM data-testid="rename-key" on:click={ let n = n3.clone(); move |_| renaming.set(Some(n.clone())) }>"Rename…"</button>
                    <button type="button" class=format!("{MENU_ITEM} text-danger") data-testid="login-remove" on:click={ let n = n4.clone(); move |_| removing.set(Some(n.clone())) }>"Remove…"</button>
                </div>
            </Show>
        </div>
    }
}

fn card(name: String, ctl: Controls) -> impl IntoView {
    let e = store().edit;
    let path = child(&base(), &name);
    let n = name.clone();
    let body = move || {
        let doc = e.doc();
        let sites = used_by(&doc, &n);
        let (sum, pass) = e
            .value(&path)
            .and_then(|v| config(&v))
            .map(|c| (summary(&c), passes(&c)))
            .unwrap_or_default();
        view! {
            <p class="mt-1 font-mono text-[13px] text-text-muted" data-testid="login-summary">{sum}</p>
            <p class="mt-0.5 text-[13px]" data-testid="login-passes">"Passes to the app: "<span class="font-mono">{pass}</span></p>
            <p class="mt-1">{site_links(sites)}</p>
        }
    };
    let menu = ctl
        .editing
        .get_untracked()
        .then(|| card_menu(name.clone(), ctl.menu, ctl.open, ctl.renaming, ctl.removing));
    let (key, shown) = (name.clone(), name.clone());
    view! {
        <li class="rounded-[11px] border border-border bg-surface px-4 py-3 dark:bg-surface-2" data-key=key.clone() data-login=key>
            <div class="flex items-center gap-2">
                <span class="font-mono font-semibold">{shown}</span>
                {menu}
            </div>
            {body}
        </li>
    }
}

#[derive(Clone, Copy)]
struct Controls {
    editing: Signal<bool>,
    menu: RwSignal<Option<String>>,
    open: RwSignal<Open>,
    renaming: RwSignal<Option<String>>,
    removing: RwSignal<Option<String>>,
}

fn remove_dialog(removing: RwSignal<Option<String>>) -> impl IntoView {
    let e = store().edit;
    let shown = RwSignal::new(false);
    Effect::new(move |_| {
        let now = removing.with(Option::is_some);
        if shown.get_untracked() != now {
            shown.set(now);
        }
    });
    Effect::new(move |_| {
        if !shown.get() && removing.with_untracked(Option::is_some) {
            removing.set(None);
        }
    });
    let title = Signal::derive(move || {
        format!(
            "Remove login service {}",
            removing.get().unwrap_or_default()
        )
    });
    let warn = move || {
        let name = removing.get()?;
        let sites = used_by(&e.doc(), &name);
        Some(if sites.is_empty() {
            view! { <p class="mb-3 text-sm text-text-muted">"No website uses it."</p> }.into_any()
        } else {
            view! { <p class="mb-3 text-sm text-warning" data-testid="login-remove-warning">{format!("{} use{} it; pick another login service or none for {} before you apply.", sites.join(", "), if sites.len() == 1 { "s" } else { "" }, if sites.len() == 1 { "it" } else { "them" })}</p> }.into_any()
        })
    };
    view! {
        <Dialog open=shown title=title tone=Tone::Danger testid="login-remove-dialog">
            {warn}
            <div class="flex justify-end gap-2">
                <Button label="Cancel" on_click=move |_| shown.set(false)/>
                <Button label="Remove" variant="danger" testid="login-remove-confirm" on_click=move |_| {
                    if let Some(n) = removing.get_untracked() {
                        e.set(&child(&base(), &n), None);
                    }
                    shown.set(false);
                }/>
            </div>
        </Dialog>
    }
}

fn rename_dialog(renaming: RwSignal<Option<String>>) -> impl IntoView {
    move || {
        renaming.get().map(|name| {
            let open = RwSignal::new(true);
            Effect::new(move |_| {
                if !open.get() && renaming.with_untracked(Option::is_some) {
                    renaming.set(None);
                }
            });
            view! { <crate::screens::create::RenameEntity entity=Entity::Login name=name open=open/> }
        })
    }
}

pub(crate) fn login_services(label: String, help: String, path: Path, spath: Path) -> AnyView {
    let s = store();
    let e = s.edit;
    let can = s.can_write_now();
    let editing = crate::form::editing_now();
    let ctl = Controls {
        editing: Signal::derive(move || can && editing.get()),
        menu: RwSignal::new(None),
        open: RwSignal::new(Open::Closed),
        renaming: RwSignal::new(None),
        removing: RwSignal::new(None),
    };
    let menu_open = RwSignal::new(false);
    Effect::new(move |_| menu_open.set(ctl.menu.with(Option::is_some)));
    Effect::new(move |_| {
        if !menu_open.get() && ctl.menu.with_untracked(Option::is_some) {
            ctl.menu.set(None);
        }
    });
    crate::app::popover(menu_open, MENU_BUTTON, MENU);
    let names = Memo::new(move |_| logins(&e.doc()).keys().cloned().collect::<Vec<_>>());
    let list = move || {
        ctl.editing.track();
        let n = names.get();
        if n.is_empty() {
            return view! { <p class="text-sm text-text-muted" data-testid="logins-none">"None yet. A login service checks each visitor of a website, like Authelia or authentik."</p> }.into_any();
        }
        view! { <ul class="grid grid-cols-1 gap-2 md:grid-cols-2" data-testid="login-cards">{n.into_iter().map(|name| card(name, ctl)).collect_view()}</ul> }.into_any()
    };
    let add = move || {
        ctl.editing.get().then(|| view! {
            <div class="mt-2">
                <Button label="Add login service" icon="plus" variant="secondary" small=true testid="login-add" on_click=move |_| ctl.open.set(Open::Add)/>
            </div>
        })
    };
    let (jp, js) = (crate::form::join(&path), crate::form::join(&spath));
    view! {
        <FieldRow id="login-services" label=label help=help path=jp schema_path=js wide=true>
            {list}
            {add}
        </FieldRow>
        {dialog::login_dialog(ctl.open)}
        {rename_dialog(ctl.renaming)}
        {remove_dialog(ctl.removing)}
    }
    .into_any()
}
