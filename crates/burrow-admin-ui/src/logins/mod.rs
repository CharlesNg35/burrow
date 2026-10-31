mod dialog;
mod kind;
mod oidc;
mod oidc_dialog;
mod site;

use burrow_admin_logic::logins::{config, passes, summary};
use burrow_admin_proto::Path;
use burrow_admin_proto::value::child;
use burrow_config::Entry;
use burrow_ui_kit::{Button, Dialog, FieldRow, Tone};
use leptos::prelude::*;

use crate::app::{MenuItem, row_menu, row_menus};
use crate::route::{Link, Route};
use crate::store::store;

pub(crate) use kind::Kind;
pub(crate) use oidc::oidc_site_editor;
pub(crate) use site::site_editor;

#[derive(Clone, PartialEq)]
pub(crate) enum Open {
    Closed,
    Add,
    Edit(String),
}

fn base() -> Path {
    Kind::Forward.base()
}

fn site_links(kind: Kind, sites: Vec<String>) -> AnyView {
    let used = kind.ids().used;
    if sites.is_empty() {
        return view! { <span class="text-xs text-text-muted" data-testid=used>"Not used by any site yet"</span> }.into_any();
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
        <span class="text-xs text-text-muted" data-testid=used>
            {burrow_admin_logic::wording::used_by_sites(n)}
            {links}
        </span>
    }
    .into_any()
}

fn card_menu(name: String, ctl: Controls) -> AnyView {
    let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
    let ids = ctl.kind.ids();
    let items = vec![
        MenuItem {
            label: "Edit…",
            testid: ids.edit,
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
            testid: ids.remove,
            danger: true,
            run: Callback::new(move |_| ctl.removing.set(Some(n3.clone()))),
        },
    ];
    row_menu(ctl.menu, name.clone(), format!("More for {name}"), items)
}

fn forward_body(name: &str) -> AnyView {
    let (sum, pass) = store()
        .edit
        .value(&child(&base(), name))
        .and_then(|v| config(&v))
        .map(|c| (summary(&c), passes(&c)))
        .unwrap_or_default();
    view! {
        <p class="mt-1 text-[13px] text-text-muted" data-testid="login-summary">{sum}</p>
        <p class="mt-0.5 text-[13px]" data-testid="login-passes">"Passes to the app: "<span class="font-mono">{pass}</span></p>
    }
    .into_any()
}

fn card(name: String, ctl: Controls) -> impl IntoView {
    let e = store().edit;
    let n = name.clone();
    let kind = ctl.kind;
    let body = move || {
        let sites = kind.used_by(&e.doc(), &n);
        view! {
            {kind.body(&n)}
            <p class="mt-1">{site_links(kind, sites)}</p>
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
    kind: Kind,
    editing: Signal<bool>,
    menu: RwSignal<Option<String>>,
    open: RwSignal<Open>,
    renaming: RwSignal<Option<String>>,
    removing: RwSignal<Option<String>>,
}

fn remove_dialog(kind: Kind, removing: RwSignal<Option<String>>) -> impl IntoView {
    let e = store().edit;
    let ids = kind.ids();
    let shown = crate::app::shown_while(removing, Option::is_some, |o| *o = None);
    let title = Signal::derive(move || {
        format!(
            "Remove {} {}",
            kind.what(),
            removing.get().unwrap_or_default()
        )
    });
    let warn = move || {
        let name = removing.get()?;
        let sites = kind.used_by(&e.doc(), &name);
        Some(if sites.is_empty() {
            view! { <p class="mb-3 text-sm text-text-muted">"No website uses it."</p> }.into_any()
        } else {
            view! { <p class="mb-3 text-sm text-warning" data-testid=ids.remove_warning>{format!("{} use{} it; pick another {} or none for {} before you apply.", sites.join(", "), if sites.len() == 1 { "s" } else { "" }, kind.what(), if sites.len() == 1 { "it" } else { "them" })}</p> }.into_any()
        })
    };
    view! {
        <Dialog open=shown title=title tone=Tone::Danger testid=ids.remove_dialog>
            {warn}
            <div class="flex justify-end gap-2">
                <Button label="Cancel" on_click=move |_| shown.set(false)/>
                <Button label="Remove" variant="danger" testid=ids.remove_confirm on_click=move |_| {
                    if let Some(n) = removing.get_untracked() {
                        e.set(&child(&kind.base(), &n), None);
                    }
                    shown.set(false);
                }/>
            </div>
        </Dialog>
    }
}

fn rename_dialog(kind: Kind, renaming: RwSignal<Option<String>>) -> impl IntoView {
    let open = crate::app::shown_while(renaming, Option::is_some, |o| *o = None);
    move || {
        renaming
            .get()
            .map(|name| view! { <crate::screens::create::RenameEntity entity=kind.entity() name=name open=open/> })
    }
}

pub(crate) fn login_services(
    kind: Kind,
    label: String,
    help: String,
    path: Path,
    spath: Path,
) -> AnyView {
    let s = store();
    let e = s.edit;
    let can = s.can_write_now();
    let editing = crate::form::editing_now();
    let ids = kind.ids();
    let ctl = Controls {
        kind,
        editing: Signal::derive(move || can && editing.get()),
        menu: row_menus(),
        open: RwSignal::new(Open::Closed),
        renaming: RwSignal::new(None),
        removing: RwSignal::new(None),
    };
    let names = Memo::new(move |_| kind.names(&e.doc()));
    let list = move || {
        ctl.editing.track();
        let n = names.get();
        if n.is_empty() {
            return view! { <p class="text-sm text-text-muted" data-testid=ids.none>"None yet"</p> }.into_any();
        }
        view! { <ul class="grid grid-cols-1 gap-2" data-testid=ids.cards>{n.into_iter().map(|name| card(name, ctl)).collect_view()}</ul> }.into_any()
    };
    let add = move || {
        ctl.editing.get().then(|| view! {
            <div class="mt-2">
                <Button label=ids.add_label icon="plus" variant="secondary" small=true testid=ids.add on_click=move |_| ctl.open.set(Open::Add)/>
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
        <FieldRow id=ids.row label=label help=help path=jp schema_path=js key_hint=key_hint table=table wide=true>
            {list}
            {add}
        </FieldRow>
        {kind.dialog(ctl.open)}
        {rename_dialog(kind, ctl.renaming)}
        {remove_dialog(kind, ctl.removing)}
    }
    .into_any()
}
