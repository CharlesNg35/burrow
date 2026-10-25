use burrow_admin_proto::Path;
use burrow_ui_kit::{Button, Icon, Tone, button_class, copy_text, tag_class};
use leptos::prelude::*;

use crate::route::{self, Route};
use crate::screens::create::singular;
use crate::screens::placement;
use crate::store::store;

use super::detail::{Parts, form_tab};

pub(super) fn head_actions(p: &Parts, can: bool) -> impl Fn() -> AnyView + use<> {
    let s = store();
    let e = s.edit;
    let Parts {
        editing,
        tab,
        tab_list,
        menu,
        renaming,
        ..
    } = p.clone();
    let (path, sec, nm) = (p.path.clone(), p.section.clone(), p.name.clone());
    let start_edit = {
        let path = path.clone();
        move || {
            e.start_edit(&path);
            if matches!(
                tab.get_untracked(),
                placement::OVERVIEW | placement::CONNECT
            ) {
                tab.set(untrack(|| form_tab(tab_list)));
            }
        }
    };
    move || {
        if editing.get() {
            return ().into_any();
        }
        let applied = crate::secrets::revealed_for(&sec, &nm).is_some().then(|| {
            view! { <span class=tag_class(Tone::Success) data-testid="applied-tag"><Icon name="check" small=true/>"Applied"</span> }
        });
        let start_edit = start_edit.clone();
        let (path, sec, nm) = (path.clone(), sec.clone(), nm.clone());
        let back = Route::List(sec.clone());
        view! {
            {applied}
            <Button
                label="Edit"
                icon="edit"
                testid="edit"
                disabled=!can
                reason=if can { String::new() } else { burrow_admin_logic::wording::needs_write_admin(burrow_admin_logic::wording::WriteAction::Edit, s.welcome.with_untracked(|w| w.as_ref().map(|w| w.you.visitor.clone())).as_deref()) }
                on_click=move |_| start_edit()
            />
            <div class="relative" class:hidden=!can>
                <button type="button" class=button_class("secondary", false) aria-label="More actions" aria-haspopup="true" aria-expanded=move || menu.get().to_string() aria-controls="entity-more-menu" id="entity-more" data-testid="more" on:click=move |_| menu.update(|m| *m = !*m)>
                    <Icon name="more"/>
                </button>
                <Show when=move || menu.get()>
                    {more_menu(&path, &nm, &back, &sec, can, menu, renaming)}
                </Show>
            </div>
        }
        .into_any()
    }
}

fn more_menu(
    path: &Path,
    nm: &str,
    back: &Route,
    sec: &str,
    can: bool,
    menu: RwSignal<bool>,
    renaming: RwSignal<bool>,
) -> impl IntoView + use<> {
    let s = store();
    let e = s.edit;
    let (path, nm, back, sec) = (path.clone(), nm.to_string(), back.clone(), sec.to_string());
    view! {
        <div class=burrow_ui_kit::menu_class("top-10 right-0 w-48", false) id="entity-more-menu" tabindex="-1" data-testid="more-menu" on:click=move |_| menu.set(false)>
            {can.then(|| {
                let path = path.clone();
                let blocked = Memo::new(move |_| e.draft.with(|d| burrow_admin_proto::value::get(d.draft(), &path).is_some_and(|v| burrow_admin_logic::grant::would_grant(None, v))));
                view! {
                    <button
                        type="button"
                        class=move || if blocked.get() { "block w-full cursor-not-allowed rounded-md px-2 py-1.5 text-left text-sm opacity-55" } else { "block w-full cursor-pointer rounded-md px-2 py-1.5 text-left text-sm hover:bg-surface-3" }
                        data-testid="rename-open"
                        disabled=move || blocked.get()
                        title=move || if blocked.get() { burrow_admin_logic::grant::IN_THE_FILE } else { "" }
                        on:click=move |_| renaming.set(true)
                    >"Rename…"</button>
                    {move || blocked.get().then(|| view! { <div class="px-2 pb-1">{crate::grants::in_the_file()}</div> })}
                }
            })}
            {can.then(|| {
                let (path, back) = (path.clone(), back.clone());
                view! { <button type="button" class="block w-full cursor-pointer rounded-md px-2 py-1.5 text-left text-sm text-danger hover:bg-surface-3" data-testid="delete" on:click=move |_| {
                    e.set(&path, None);
                    route::go(back.clone());
                }>{format!("Delete {}…", singular(&sec))}</button> }
            })}
            {
                let nm = nm.clone();
                view! { <button type="button" class="block w-full cursor-pointer rounded-md px-2 py-1.5 text-left text-sm hover:bg-surface-3" on:click=move |_| { copy_text(&nm); s.toast(Tone::Success, "Copied", None); }>"Copy name"</button> }
            }
        </div>
    }
}

pub(super) fn save_bar(
    p: &Parts,
    changed: Signal<usize>,
    blocker: Signal<Option<&'static str>>,
) -> impl Fn() -> Option<AnyView> + Clone + use<> {
    let s = store();
    let e = s.edit;
    let Parts {
        editing, problems, ..
    } = p.clone();
    let path = p.path.clone();
    let discard = {
        let path = path.clone();
        move || e.discard_edit(&path)
    };
    let save = {
        let path = path.clone();
        move || {
            e.stop_edit(&path);
            s.toast(Tone::Success, "Added to changes", Some("Review"));
        }
    };
    move || {
        editing.get().then(|| {
            let discard = discard.clone();
            let save = save.clone();
            let p = problems.get();
            let mut fields: Vec<String> = p
                .iter()
                .map(|x| {
                    if x.label.is_empty() {
                        x.message.clone()
                    } else {
                        x.label.clone()
                    }
                })
                .collect();
            fields.dedup();
            crate::screens::bar::save_bar(crate::screens::bar::SaveBar {
                count: p.len(),
                problems: fields,
                changed,
                blocker,
                on_discard: Callback::new(move |_| discard()),
                on_save: Callback::new(move |_| save()),
            })
        })
    }
}
