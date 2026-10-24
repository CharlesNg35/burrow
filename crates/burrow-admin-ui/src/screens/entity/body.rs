use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Node;
use burrow_admin_proto::value::get;
use burrow_config::{ClientEntry, VisitorEntry};
use burrow_ui_kit::PathData;
use burrow_ui_kit::{Button, EmptyState, PathDiagram, Tabs, button_class};
use leptos::prelude::*;

use crate::route::{Link, Route};
use crate::screens::create::singular;
use crate::screens::placement;
use crate::store::store;

use super::detail::Parts;
use super::{
    CLIENTS, VISITORS, apply_notes, capital, overview, plural, problem_list, served,
    visitor_overview,
};

pub(super) fn body(
    p: &Parts,
    path_data: Signal<PathData>,
    tab_counts: Signal<Vec<String>>,
    tab_errors: Signal<Vec<usize>>,
    what: &'static str,
) -> impl Fn() -> Option<AnyView> + use<> {
    let s = store();
    let Parts {
        editing,
        exists,
        tab,
        tab_list,
        problems,
        ..
    } = p.clone();
    let (sec, nm, path) = (p.section.clone(), p.name.clone(), p.path.clone());
    move || {
        let (sec, nm, path) = (sec.clone(), nm.clone(), path.clone());
        let schema = s.schema.get()?;
        if !exists.get() {
            return Some(missing(&sec, &nm, &path));
        }
        let node = schema.root.at(&path)?.clone();
        let tab_body = tab_body(&node, &sec, &nm, &path, tab);
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
}

fn missing(sec: &str, nm: &str, path: &Path) -> AnyView {
    let e = store().edit;
    let (sec, nm) = (sec.to_string(), nm.to_string());
    let back = Route::List(sec.clone());
    view! {
        {
            let was = e.draft.with_untracked(|d| get(d.base(), path).cloned());
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
    .into_any()
}

fn tab_body(
    node: &Node,
    sec: &str,
    nm: &str,
    path: &Path,
    tab: RwSignal<&'static str>,
) -> impl Fn() -> AnyView + use<> {
    let node = node.clone();
    let spath = vec![sec.to_string(), "*".to_string()];
    let (sec2, nm2, path2) = (sec.to_string(), nm.to_string(), path.clone());
    move || {
        let t = tab.get();
        match (sec2.as_str(), t) {
            (CLIENTS, placement::OVERVIEW) => served(&nm2).into_any(),
            (_, placement::OVERVIEW) => overview(&sec2, &nm2, false).into_any(),
            (CLIENTS, placement::SECRETS) => {
                crate::secrets::secret_list(ClientEntry::SECRET.of(&nm2), "secrets".into(), 4)
            }
            (VISITORS, placement::SECRETS) => {
                crate::secrets::secret_list(VisitorEntry::SECRET.of(&nm2), "secrets".into(), 4)
            }
            (_, placement::CONNECT) => {
                if crate::secrets::revealed_for(&sec2, &nm2).is_some() {
                    view! { <crate::secrets::ShownOnce section=sec2.clone() name=nm2.clone()/> }
                        .into_any()
                } else {
                    view! { <crate::secrets::ConnectPanel section=sec2.clone() name=nm2.clone()/> }
                        .into_any()
                }
            }
            (_, t) => {
                let skip: Vec<&str> = burrow_admin_logic::tree::fields(&node)
                    .iter()
                    .map(|f| f.key.as_str())
                    .filter(|k| placement::tab_of(&sec2, k) != t)
                    .collect();
                let may =
                    (sec2 == VISITORS && t == placement::ACCESS).then(|| visitor_overview(&nm2));
                view! {
                    {may}
                    <div data-form-tab=t>
                        {crate::form::object_view_skipping(&node, path2.clone(), spath.clone(), path2.clone(), &skip)}
                    </div>
                }
                .into_any()
            }
        }
    }
}
