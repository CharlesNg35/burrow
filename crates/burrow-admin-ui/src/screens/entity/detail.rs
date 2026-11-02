use burrow_admin_logic::changeset::ops_between;
use burrow_admin_proto::Path;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::get;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use burrow_ui_kit::PathData;
use burrow_ui_kit::{Icon, Tone, tag_class};
use leptos::context::Provider;
use leptos::prelude::*;

use crate::form::{EditMode, plain};
use crate::format;
use crate::route::{self, Route};
use crate::screens::DetailPane;
use crate::screens::create::{RenameEntity, singular};
use crate::screens::paths;
use crate::screens::placement;
use crate::store::store;

use super::actions::{head_actions, save_bar};
use super::body::body;
use super::problems::Problem;
use super::{
    CLIENTS, SERVICES, VISITORS, plural, problems_for, problems_of, secrets, wrapping_tag,
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
    track_editing(editing, path.clone());
    let tab_list = tab_list_of(&section);
    let tab = tab_signal(&section, &name, tab_list, fresh);
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
    let path_data = path_data(&section, &name, editing);
    let protocol_tag = protocol_tag(&section, &name);
    let file_tag = super::file::file_tag(path.clone());
    let read_only_note = super::file::read_only_note(path.clone());
    let tab_counts = tab_counts(&section, &name, tab_list);
    let tab_errors = Signal::derive(move || {
        let p = problems.get();
        tab_list.with(|t| {
            t.iter()
                .map(|(k, _)| p.iter().filter(|x| x.tab == *k).count())
                .collect::<Vec<usize>>()
        })
    });
    let locked = s.item_locked(&path);
    let can = s.can_write_now() && locked.is_none();
    let parts = Parts {
        locked,
        section: section.clone(),
        name: name.clone(),
        path: path.clone(),
        editing,
        exists,
        tab,
        tab_list,
        problems,
        menu,
        renaming,
    };
    let head_actions = head_actions(&parts, can);
    let body = body(&parts, path_data, tab_counts, tab_errors, what);
    let save_bar = save_bar(&parts, changed, blocker);
    let title_dot = title_dot(&section, &name);
    let crumb = Route::List(section.clone());
    let (ent_r, name_r) = (crate::screens::entity_of(&section), name.clone());
    let (sec_d, name_d, plural_s) = (section.clone(), name.clone(), plural(&section));
    view! {
        <Provider value=mode>
        <DetailPane back=crumb.clone()>
            {crate::screens::crumbs(
                crumb,
                plural_s.to_string(),
                name.clone(),
                Signal::derive(move || !editing.get()),
                Some(view! { {move || editing.get().then(|| view! { <span aria-hidden="true">"/"</span><span class="font-semibold text-text">"Edit"</span> })} }.into_any()),
            )}
            <burrow_ui_kit::PageHeader
                phone_hidden=Signal::derive(move || editing.get())
                hidden=Signal::derive(move || !exists.get())
                actions={view! { {head_actions} }.into_any()}
            >
                {title_dot}{name.clone()}{protocol_tag}{file_tag}
            </burrow_ui_kit::PageHeader>
            {read_only_note}
            <RenameEntity entity=ent_r name=name_r open=renaming/>
            <div data-testid="detail" data-entity=sec_d data-name=name_d>{body}</div>
            {move || exists.get().then(save_bar.clone())}
        </DetailPane>
        </Provider>
    }
}

#[derive(Clone)]
pub(super) struct Parts {
    pub(super) section: String,
    pub(super) name: String,
    pub(super) path: Path,
    pub(super) editing: Memo<bool>,
    pub(super) exists: Memo<bool>,
    pub(super) tab: RwSignal<&'static str>,
    pub(super) tab_list: Memo<Vec<(&'static str, String)>>,
    pub(super) problems: Memo<Vec<Problem>>,
    pub(super) menu: RwSignal<bool>,
    pub(super) renaming: RwSignal<bool>,
    pub(super) locked: Option<String>,
}

fn track_editing(editing: Memo<bool>, path: Path) {
    let s = store();
    let e = s.edit;
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
}

fn tab_list_of(section: &str) -> Memo<Vec<(&'static str, String)>> {
    let s = store();
    let section = section.to_string();
    let node_now = {
        let sec = section.clone();
        Memo::new(move |_| {
            s.schema.with(|sc| {
                sc.as_ref()
                    .and_then(|sc| sc.root.at(&[sec.clone(), "*".to_string()]).cloned())
            })
        })
    };
    let sec = section.clone();
    Memo::new(move |_| {
        node_now.with(|n| match n {
            Some(n) => placement::tabs(&sec, n),
            None => vec![(placement::OVERVIEW, "Overview".to_string())],
        })
    })
}

pub(super) fn form_tab(tab_list: Memo<Vec<(&'static str, String)>>) -> &'static str {
    tab_list.with(|t| {
        t.iter()
            .map(|(k, _)| *k)
            .find(|k| matches!(*k, placement::ACCESS | placement::SETTINGS))
            .unwrap_or(placement::SETTINGS)
    })
}

fn tab_signal(
    section: &str,
    name: &str,
    tab_list: Memo<Vec<(&'static str, String)>>,
    fresh: bool,
) -> RwSignal<&'static str> {
    let (section, name) = (section.to_string(), name.to_string());
    let asked = route::query(route::TAB_QUERY);
    let known = tab_list.with_untracked(|t| {
        t.iter()
            .map(|(k, _)| *k)
            .find(|k| asked.as_deref() == Some(*k))
    });
    let tab: RwSignal<&'static str> = RwSignal::new(match known {
        Some(k) => k,
        None if fresh => untrack(|| form_tab(tab_list)),
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
            route::set_query(route::TAB_QUERY, t);
        }
    });
    tab
}

fn path_data(section: &str, name: &str, editing: Memo<bool>) -> Signal<PathData> {
    let s = store();
    let e = s.edit;
    let (sec, nm) = (section.to_string(), name.to_string());
    Signal::derive(move || {
        let status = s.status.get();
        match sec.as_str() {
            CLIENTS => {
                let listen = status
                    .as_ref()
                    .and_then(|st| st.listening.first().map(|l| format::listen_addr(l)))
                    .unwrap_or_default();
                e.draft
                    .with(|d| paths::client(&nm, d.draft(), status.as_deref(), &listen))
            }
            VISITORS => {
                let me = s.welcome.with(|w| {
                    w.as_ref()
                        .map(|w| (w.you.visitor.clone(), w.you.from.clone()))
                });
                e.draft.with(|d| {
                    paths::visitor(
                        &nm,
                        d.draft(),
                        status.as_deref(),
                        me.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
                    )
                })
            }
            _ => {
                let problems = problems_for(&vec![sec.clone(), nm.clone()]);
                let editing = editing.get();
                let reserved = crate::domain::reserved_clients();
                e.draft.with(|d| {
                    let svc = get(d.draft(), &[sec.clone(), nm.clone()]).unwrap_or(&Value::Null);
                    paths::service(&nm, svc, status.as_deref(), &problems, editing, &reserved)
                })
            }
        }
    })
}

fn protocol_tag(section: &str, name: &str) -> impl Fn() -> AnyView + Clone + use<> {
    let s = store();
    let e = s.edit;
    let (sec, nm) = (section.to_string(), name.to_string());
    move || {
        match sec.as_str() {
        SERVICES => {
            let svc = ServiceEntry::entry(&e.doc(), &nm).cloned().unwrap_or_default();
            super::protocol_view(&svc).into_any()
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
}

fn tab_counts(
    section: &str,
    name: &str,
    tab_list: Memo<Vec<(&'static str, String)>>,
) -> Signal<Vec<String>> {
    let s = store();
    let e = s.edit;
    let (sec, nm) = (section.to_string(), name.to_string());
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
                        (VISITORS, placement::SECRETS) => secrets(VisitorEntry::SECRET.get(&me)),
                        _ => 0,
                    };
                    if n > 0 { n.to_string() } else { String::new() }
                })
                .collect()
        })
    })
}

fn title_dot(section: &str, name: &str) -> impl IntoView + use<> {
    let s = store();
    let (sec, nm) = (section.to_string(), name.to_string());
    move || {
        (sec == CLIENTS).then(|| {
            let on = s.status.with(|st| st.as_ref().and_then(|st| st.clients.iter().find(|c| c.name == nm)).is_some_and(|c| c.connected));
            view! { <span class=format!("size-2.5 shrink-0 rounded-full {}", if on { "bg-success" } else { "bg-transparent ring-[1.5px] ring-text-faint ring-inset" }) aria-label=if on { burrow_admin_logic::wording::ONLINE } else { burrow_admin_logic::wording::OFFLINE }></span> }
        })
    }
}
