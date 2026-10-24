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
                reason=if can { String::new() } else { format!("Editing needs write admin. {} has read.", s.welcome.with_untracked(|w| w.as_ref().map(|w| w.you.visitor.clone()).unwrap_or_default())) }
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
        <div class="absolute top-10 right-0 z-30 w-48 rounded-[10px] border border-border-strong bg-surface p-1 shadow-overlay outline-none dark:bg-surface-2" id="entity-more-menu" tabindex="-1" data-testid="more-menu" on:click=move |_| menu.set(false)>
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
            let bad = !p.is_empty();
            let mut fields: Vec<String> = p.iter().map(|x| if x.label.is_empty() { x.message.clone() } else { x.label.clone() }).collect();
            fields.dedup();
            view! {
                <div class=format!("sticky bottom-4 z-20 mt-6 flex items-center gap-3 rounded-card border bg-surface px-4 py-3 shadow-overlay {}", if bad { "border-danger" } else { "border-border" }) data-testid="save-bar" data-sticky-bar="">
                    {if bad {
                        view! { <span class="flex min-w-0 items-center gap-1.5 text-sm font-semibold text-danger"><Icon name="alert-circle"/>{format!("{} problem{}", p.len(), if p.len() == 1 { "" } else { "s" })}<span class="truncate font-normal text-text-muted">{format!(" · {}", fields.join(", "))}</span></span> }.into_any()
                    } else {
                        view! { <span class="text-sm text-text-muted" data-testid="fields-changed">{move || { let n = changed.get(); match blocker.get() { Some(b) => b.to_string(), None if n == 0 => "No changes yet".to_string(), None => format!("{n} field{} changed", if n == 1 { "" } else { "s" }) } }}</span> }.into_any()
                    }}
                    <span class="ml-auto"></span>
                    <Button label=Signal::derive(move || if changed.get() == 0 { "Close".to_string() } else { "Discard".to_string() }) variant="ghost" testid="discard-edit" on_click=move |_| discard()/>
                    <Button
                        label="Add to changes"
                        variant="primary"
                        testid="save"
                        disabled=Signal::derive(move || bad || changed.get() == 0 || blocker.get().is_some())
                        reason=Signal::derive(move || if bad { "Fix the problems first".to_string() } else if let Some(b) = blocker.get() { b.to_string() } else if changed.get() == 0 { "Nothing changed yet".to_string() } else { String::new() })
                        on_click=move |_| save()
                    />
                </div>
            }
            .into_any()
        })
    }
}
