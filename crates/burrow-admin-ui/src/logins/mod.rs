mod dialog;
mod site;

use burrow_admin_logic::logins::{config, logins, passes, summary, used_by};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::value::child;
use burrow_config::{Entry, entity_path};
use burrow_ui_kit::{Button, Dialog, FieldRow, Tone};
use leptos::prelude::*;

use crate::app::{MenuItem, row_menu, row_menus};
use crate::route::{Link, Route};
use crate::store::store;

pub(crate) use site::site_editor;

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

fn card_menu(name: String, ctl: Controls) -> AnyView {
    let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
    let items = vec![
        MenuItem {
            label: "Edit…",
            testid: "login-edit",
            danger: false,
            run: Callback::new(move |_| ctl.open.set(Open::Edit(n1.clone()))),
        },
        MenuItem {
            label: "Rename…",
            testid: "rename-key",
            danger: false,
            run: Callback::new(move |_| ctl.renaming.set(Some(n2.clone()))),
        },
        MenuItem {
            label: "Remove…",
            testid: "login-remove",
            danger: true,
            run: Callback::new(move |_| ctl.removing.set(Some(n3.clone()))),
        },
    ];
    row_menu(ctl.menu, name.clone(), format!("More for {name}"), items)
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
            <p class="mt-1 text-[13px] text-text-muted" data-testid="login-summary">{sum}</p>
            <p class="mt-0.5 text-[13px]" data-testid="login-passes">"Passes to the app: "<span class="font-mono">{pass}</span></p>
            <p class="mt-1">{site_links(sites)}</p>
        }
    };
    let menu = ctl
        .editing
        .get_untracked()
        .then(|| view! { <div class="ml-auto">{card_menu(name.clone(), ctl)}</div> });
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
        menu: row_menus(),
        open: RwSignal::new(Open::Closed),
        renaming: RwSignal::new(None),
        removing: RwSignal::new(None),
    };
    let names = Memo::new(move |_| logins(&e.doc()).keys().cloned().collect::<Vec<_>>());
    let list = move || {
        ctl.editing.track();
        let n = names.get();
        if n.is_empty() {
            return view! { <p class="text-sm text-text-muted" data-testid="logins-none">"None yet"</p> }.into_any();
        }
        view! { <ul class="grid grid-cols-1 gap-2" data-testid="login-cards">{n.into_iter().map(|name| card(name, ctl)).collect_view()}</ul> }.into_any()
    };
    let add = move || {
        ctl.editing.get().then(|| view! {
            <div class="mt-2">
                <Button label="Add login service" icon="plus" variant="secondary" small=true testid="login-add" on_click=move |_| ctl.open.set(Open::Add)/>
            </div>
        })
    };
    let (jp, js) = (crate::form::join(&path), crate::form::join(&spath));
    let table = use_context::<crate::form::ShowKeys>().is_some_and(|k| k.0);
    let key_hint = if table {
        path.join(".\u{200b}")
    } else {
        String::new()
    };
    view! {
        <FieldRow id="login-services" label=label help=help path=jp schema_path=js key_hint=key_hint table=table wide=true>
            {list}
            {add}
        </FieldRow>
        {dialog::login_dialog(ctl.open)}
        {rename_dialog(ctl.renaming)}
        {remove_dialog(ctl.removing)}
    }
    .into_any()
}
