use burrow_admin_logic::changeset::ops_between;
use burrow_admin_proto::Path;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::get;
use burrow_config::{ClientEntry, Entry, Protocol, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{
    Button, EmptyState, Icon, PathDiagram, Tabs, Tone, button_class, copy_text, tag_class,
};
use leptos::context::Provider;
use leptos::prelude::*;

use crate::form::{EditMode, plain};
use crate::format;
use crate::route::{self, Link, Route};
use crate::screens::DetailPane;
use crate::screens::create::{RenameEntity, singular};
use crate::screens::paths;
use crate::screens::placement;
use crate::store::store;

use super::{
    CLIENTS, SERVICES, VISITORS, apply_notes, capital, overview, plural, problem_list,
    problems_for, problems_of, secrets, served, visitor_overview, wrapping_tag,
};

#[component]
pub(super) fn Detail(section: String, name: String) -> impl IntoView {
    let s = store();
    let e = s.edit;
    let path: Path = vec![section.clone(), name.clone()];
    let exists = {
        let path = path.clone();
        Memo::new(move |_| e.draft.with(|d| get(d.draft(), &path).is_some()))
    };
    let fresh = e.draft.with_untracked(|d| {
        get(d.base(), &path).is_none()
            && get(d.draft(), &path).is_some()
            && d.renamed_from(&section, &name).is_none()
    }) && untrack(|| crate::secrets::revealed_for(&section, &name)).is_none();
    if fresh && s.can_write_now() {
        e.start_edit(&path);
    }
    let editing = {
        let path = path.clone();
        Memo::new(move |_| e.is_editing(&path))
    };
    Effect::new(move |_| {
        let on = editing.get();
        let _ = s.editing.try_set(on);
        if on {
            request_animation_frame(crate::screens::clear_of_bar);
        }
    });
    {
        let path = path.clone();
        on_cleanup(move || {
            let _ = s.editing.try_set(false);
            request_animation_frame(crate::screens::clear_of_bar);
            if !untrack(|| e.unsaved(&path)) {
                e.stop_edit(&path);
            }
        });
    }
    let node_now = {
        let sec = section.clone();
        Memo::new(move |_| {
            s.schema.with(|sc| {
                sc.as_ref()
                    .and_then(|sc| sc.root.at(&[sec.clone(), "*".to_string()]).cloned())
            })
        })
    };
    let tab_list = {
        let sec = section.clone();
        Memo::new(move |_| {
            node_now.with(|n| match n {
                Some(n) => placement::tabs(&sec, n),
                None => vec![(placement::OVERVIEW, "Overview".to_string())],
            })
        })
    };
    let form_tab = move || {
        tab_list.with(|t| {
            t.iter()
                .map(|(k, _)| *k)
                .find(|k| matches!(*k, placement::ACCESS | placement::SETTINGS))
                .unwrap_or(placement::SETTINGS)
        })
    };
    let asked = route::query("tab");
    let known = tab_list.with_untracked(|t| {
        t.iter()
            .map(|(k, _)| *k)
            .find(|k| asked.as_deref() == Some(*k))
    });
    let tab: RwSignal<&'static str> = RwSignal::new(match known {
        Some(k) => k,
        None if fresh => untrack(form_tab),
        None if untrack(|| crate::secrets::revealed_for(&section, &name)).is_some() => {
            placement::CONNECT
        }
        None => {
            tab_list.with_untracked(|t| t.first().map(|(k, _)| *k).unwrap_or(placement::OVERVIEW))
        }
    });
    {
        let (sec, nm) = (section.clone(), name.clone());
        Effect::new(move |was: Option<bool>| {
            let now = crate::secrets::revealed_for(&sec, &nm).is_some();
            if now && was == Some(false) {
                tab.set(placement::CONNECT);
            }
            now
        });
    }
    Effect::new(move |first: Option<()>| {
        let t = tab.get();
        if first.is_some() {
            route::set_query("tab", t);
        }
    });
    let mode = EditMode(Signal::derive(move || editing.get()));
    let renaming = RwSignal::new(false);
    let menu = RwSignal::new(false);
    crate::app::popover(menu, "entity-more", "entity-more-menu");
    let what = singular(&section);
    if section == ClientEntry::section()
        && let Ok(h) = set_interval_with_handle(
            move || crate::api::load_state(s),
            std::time::Duration::from_secs(5),
        )
    {
        on_cleanup(move || h.clear());
    }
    let start_edit = {
        let path = path.clone();
        move || {
            e.start_edit(&path);
            if matches!(
                tab.get_untracked(),
                placement::OVERVIEW | placement::CONNECT
            ) {
                tab.set(untrack(form_tab));
            }
        }
    };
    let changed = {
        let path = path.clone();
        Signal::derive(move || {
            let now = e.value(&path).unwrap_or(Value::Null);
            e.editing.with(|m| {
                let snap = m.get(&path).cloned().flatten().unwrap_or(Value::Null);
                ops_between(&snap, &now).len()
            })
        })
    };
    let problems = {
        let (path, sec) = (path.clone(), section.clone());
        Memo::new(move |_| problems_of(&sec, &path))
    };
    let blocker = {
        let (path, sec) = (path.clone(), section.clone());
        Signal::derive(move || {
            (sec == SERVICES)
                .then(|| e.value(&path).as_ref().and_then(paths::add_blocker))
                .flatten()
        })
    };
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
    let (sec, nm) = (section.clone(), name.clone());
    let path_data = Signal::derive(move || {
        let doc = e.doc();
        let status = s.status.get();
        match sec.as_str() {
            CLIENTS => {
                let listen = status
                    .as_ref()
                    .and_then(|st| st.listening.first().map(|l| format::listen_addr(l)))
                    .unwrap_or_default();
                paths::client(&nm, &doc, status.as_deref(), &listen)
            }
            VISITORS => {
                let me = s.welcome.with(|w| {
                    w.as_ref()
                        .map(|w| (w.you.visitor.clone(), w.you.from.clone()))
                });
                paths::visitor(
                    &nm,
                    &doc,
                    status.as_deref(),
                    me.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
                )
            }
            _ => {
                let svc = get(&doc, &[sec.clone(), nm.clone()])
                    .cloned()
                    .unwrap_or(Value::Null);
                paths::service(
                    &nm,
                    &svc,
                    status.as_deref(),
                    &problems_for(&vec![sec.clone(), nm.clone()]),
                    editing.get(),
                    &crate::domain::reserved_clients(),
                )
            }
        }
    });
    let protocol_tag = {
        let (sec, nm) = (section.clone(), name.clone());
        move || {
            match sec.as_str() {
            SERVICES => {
                let p = ServiceEntry::PROTOCOL
                    .at(&e.doc(), &nm)
                    .map(plain)
                    .unwrap_or_else(|| Protocol::default().wire().into());
                let tone = if p == Protocol::Udp.wire() {
                    Tone::Violet
                } else {
                    Tone::Accent
                };
                view! { <span class=tag_class(tone)>{p.to_uppercase()}</span> }.into_any()
            }
            VISITORS => {
                let admin = VisitorEntry::ADMIN.at(&e.doc(), &nm).map(plain);
                let me = s
                    .welcome
                    .with(|w| w.as_ref().is_some_and(|w| w.you.visitor == nm));
                view! {
                    {admin.map(|a| {
                        let write = a == burrow_config::admin_schema::AdminEnum::wire(&burrow_admin_proto::msg::AdminLevel::Write);
                        let tone = if write { Tone::Danger } else { Tone::Accent };
                        view! { <span class=tag_class(tone) data-testid="admin-tag"><Icon name=if write { "shield" } else { "eye" } small=true/>{format!("admin · {a}")}</span> }
                    })}
                    {me.then(|| view! { <span class=tag_class(Tone::Neutral) data-testid="you-tag">"you"</span> })}
                }
                .into_any()
            }
            CLIENTS => s
                .status
                .with(|st| format::limit_tag(st.as_deref(), &nm))
                .map(|(text, why)| {
                    view! { <span class=wrapping_tag(Tone::Warning) title=why data-testid="limit-tag">{text}</span> }
                })
                .into_any(),
            _ => ().into_any(),
        }
        }
    };
    let tab_counts = {
        let (sec, nm) = (section.clone(), name.clone());
        Signal::derive(move || {
            let doc = e.doc();
            let me = doc
                .get(&sec)
                .and_then(|m| m.get(&nm))
                .cloned()
                .unwrap_or(Value::Null);
            tab_list.with(|t| {
                t.iter()
                    .map(|(k, _)| {
                        let n = match (sec.as_str(), *k) {
                            (CLIENTS, placement::SECRETS) => secrets(ClientEntry::SECRET.get(&me)),
                            (VISITORS, placement::SECRETS) => {
                                secrets(VisitorEntry::SECRET.get(&me))
                            }
                            _ => 0,
                        };
                        if n > 0 { n.to_string() } else { String::new() }
                    })
                    .collect()
            })
        })
    };
    let tab_errors = Signal::derive(move || {
        let p = problems.get();
        tab_list.with(|t| {
            t.iter()
                .map(|(k, _)| p.iter().filter(|x| x.tab == *k).count())
                .collect::<Vec<usize>>()
        })
    });
    let can = s.can_write_now();
    let head_actions = {
        let start_edit = start_edit.clone();
        let (path, sec, nm) = (path.clone(), section.clone(), name.clone());
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
                        {
                            let (path, nm, back, sec) = (path.clone(), nm.clone(), back.clone(), sec.clone());
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
                    </Show>
                </div>
            }
            .into_any()
        }
    };
    let body = {
        let (sec, nm, path) = (section.clone(), name.clone(), path.clone());
        move || {
            let (sec, nm, path) = (sec.clone(), nm.clone(), path.clone());
            let schema = s.schema.get()?;
            if !exists.get() {
                let back = Route::List(sec.clone());
                return Some(
                    view! {
                        {
                            let was = e.draft.with_untracked(|d| get(d.base(), &path).cloned());
                            let removed = was.is_some();
                            let p2 = path.clone();
                            view! {
                                <EmptyState icon="trash" title=if removed { format!("{} {nm} is removed in your changes", capital(singular(&sec))) } else { format!("No {} named {nm}", singular(&sec)) } message=if removed { "It goes when you apply. Undo keeps it." } else { "It may have been removed or renamed." } testid="detail-missing">
                                    {removed.then(|| view! { <Button label="Undo the removal" variant="secondary" icon="revert" testid="undo-remove" on_click=move |_| e.set(&p2, was.clone())/> })}
                                    <Link to=back class=button_class("secondary", false)>{format!("Back to {}", plural(&sec))}</Link>
                                </EmptyState>
                            }
                        }
                    }
                    .into_any(),
                );
            }
            let node = schema.root.at(&path)?.clone();
            let spath = vec![sec.clone(), "*".to_string()];
            let (sec2, nm2, path2) = (sec.clone(), nm.clone(), path.clone());
            let tab_body = move || {
                let t = tab.get();
                match (sec2.as_str(), t) {
                    (CLIENTS, placement::OVERVIEW) => served(&nm2).into_any(),
                    (_, placement::OVERVIEW) => overview(&sec2, &nm2, false).into_any(),
                    (CLIENTS, placement::SECRETS) => crate::secrets::secret_list(
                        ClientEntry::SECRET.of(&nm2),
                        "secrets".into(),
                        4,
                    ),
                    (VISITORS, placement::SECRETS) => crate::secrets::secret_list(
                        VisitorEntry::SECRET.of(&nm2),
                        "secrets".into(),
                        4,
                    ),
                    (_, placement::CONNECT) => {
                        if crate::secrets::revealed_for(&sec2, &nm2).is_some() {
                            view! { <crate::secrets::ShownOnce section=sec2.clone() name=nm2.clone()/> }.into_any()
                        } else {
                            view! { <crate::secrets::ConnectPanel section=sec2.clone() name=nm2.clone()/> }.into_any()
                        }
                    }
                    (_, t) => {
                        let skip: Vec<&str> = burrow_admin_logic::tree::fields(&node)
                            .iter()
                            .map(|f| f.key.as_str())
                            .filter(|k| placement::tab_of(&sec2, k) != t)
                            .collect();
                        let may = (sec2 == VISITORS && t == placement::ACCESS)
                            .then(|| visitor_overview(&nm2));
                        view! {
                            {may}
                            <div data-form-tab=t>
                                {crate::form::object_view_skipping(&node, path2.clone(), spath.clone(), path2.clone(), &skip)}
                            </div>
                        }
                        .into_any()
                    }
                }
            };
            let nm_p = nm.clone();
            Some(view! {
                <div class:hidden=move || editing.get()>
                    <PathDiagram data=path_data testid="path-diagram"/>
                    {overview(&sec, &nm, true)}
                </div>
                {move || view! { <Tabs tabs=tab_list.get() active=tab counts=tab_counts errors=tab_errors/> }}
                <div data-testid="detail-settings">
                    {move || (editing.get() && !problems.get().is_empty()).then(|| problem_list(problems.get(), what, nm_p.clone(), tab))}
                    {move || editing.get().then(apply_notes).flatten()}
                    {tab_body}
                </div>
            }
            .into_any())
        }
    };
    let save_bar = {
        let discard = discard.clone();
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
    };
    let title_dot = {
        let (sec, nm) = (section.clone(), name.clone());
        move || {
            (sec == CLIENTS).then(|| {
                let on = s.status.with(|st| st.as_ref().and_then(|st| st.clients.iter().find(|c| c.name == nm)).is_some_and(|c| c.connected));
                view! { <span class=format!("size-2.5 shrink-0 rounded-full {}", if on { "bg-success" } else { "bg-transparent ring-[1.5px] ring-text-faint ring-inset" }) aria-label=if on { "online" } else { "offline" }></span> }
            })
        }
    };
    let crumb = Route::List(section.clone());
    let (sec_r, name_r) = (section.clone(), name.clone());
    let (sec_d, name_d, plural_s) = (section.clone(), name.clone(), plural(&section));
    view! {
        <Provider value=mode>
        <DetailPane back=crumb.clone()>
            <nav class="mb-1.5 flex gap-1.5 text-sm text-text-muted" class:max-sm:hidden=move || editing.get() aria-label="Breadcrumb">
                <Link to=crumb class="text-text-muted no-underline hover:text-text">{plural_s}</Link>
                <span aria-hidden="true">"/"</span>
                <span class="font-semibold text-text">{name.clone()}</span>
                {move || editing.get().then(|| view! { <span aria-hidden="true">"/"</span><span class="font-semibold text-text">"Edit"</span> })}
            </nav>
            <header class="mb-[22px] flex items-start gap-3.5" class:max-sm:hidden=move || editing.get() class:hidden=move || !exists.get()>
                <div class="min-w-0 flex-1">
                    <h1 class="flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[25px] font-bold tracking-[-0.015em]">{title_dot}{name.clone()}{protocol_tag}</h1>

                </div>
                <div class="flex items-center gap-2">{head_actions}</div>
            </header>
            <RenameEntity section=sec_r name=name_r open=renaming/>
            <div data-testid="detail" data-entity=sec_d data-name=name_d>{body}</div>
            {move || exists.get().then(save_bar.clone())}
        </DetailPane>
        </Provider>
    }
}
