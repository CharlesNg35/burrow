use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_config::{Entry, Protocol, ServiceEntry, entity_of_section};
use burrow_ui_kit::{EmptyState, Tone};
use leptos::prelude::*;

use crate::entities::singular;
use crate::route::{self, Route};
use crate::screens::create::{NewEntity, NewWebsite};
use crate::screens::{DetailPane, ListPane, NewChoice, Row, master_detail, wide};
use crate::store::store;
use burrow_admin_proto::status::Status;
use std::sync::Arc;

use super::file::Files;
use super::index::Index;
use super::live::{self, Classes};
use super::rows::{client_row, service_row, visitor_row};
use super::{Detail, plural};

#[derive(Clone, PartialEq, Default)]
pub(super) struct Base {
    rows: Vec<Row>,
    names: Vec<(String, Vec<String>)>,
}

fn me() -> Option<String> {
    store()
        .welcome
        .with(|w| w.as_ref().map(|w| w.you.visitor.clone()))
}

fn one_row(
    section: &str,
    name: &str,
    v: &Value,
    base_items: Option<&burrow_admin_proto::serde_json::Map<String, Value>>,
    ix: &Index,
) -> Row {
    let s = store();
    let in_base = base_items.and_then(|b| b.get(name));
    let was = s
        .edit
        .draft
        .with(|d| d.base_name(section, name))
        .map(|w| vec![section.to_string(), w]);
    let pending = was.is_none().then_some("new");
    let entity_path = vec![section.to_string(), name.to_string()];
    let mark = if s.edit.unsaved(&entity_path) {
        Some((
            burrow_admin_logic::wording::NOT_ADDED.to_string(),
            Tone::Warning,
        ))
    } else if pending.is_none() && in_base != Some(v) {
        Some((
            burrow_admin_logic::wording::IN_CHANGES.to_string(),
            Tone::Accent,
        ))
    } else {
        None
    };
    let to = Route::Detail(section.to_string(), name.to_string());
    let mut row = match entity_of_section(section) {
        Some(Entity::Client) => client_row(name, ix, pending, to),
        Some(Entity::Visitor) => visitor_row(name, v, ix, pending, to),
        _ => service_row(name, (v, ix.https_on), pending, to),
    };
    if let Some(m) = mark {
        row.tags.insert(0, m);
    }
    row.file = ix
        .files
        .item_file(&entity_path, was.as_ref())
        .unwrap_or_default();
    row.problems = super::problems::problems_of(section, &entity_path).len();
    if let Some((file, locked)) = ix.files.label(&entity_path, was.as_ref()) {
        row.terms.push('\n');
        row.terms.push_str(&file);
        if locked || ix.files.main != file {
            row.kept = Some(if locked {
                crate::screens::Kept::ReadOnly(file)
            } else {
                crate::screens::Kept::File(file)
            });
        }
    }
    row
}

fn clients_for(section: &str, v: &Value) -> Vec<String> {
    match entity_of_section(section) {
        Some(Entity::Client | Entity::Visitor) => Vec::new(),
        _ => crate::screens::paths::clients_of(v),
    }
}

type Seen = Option<(Option<Arc<Status>>, Option<String>, (usize, usize))>;

fn classes_now(
    section: &str,
    names: &[(String, Vec<String>)],
    prev: Option<&Classes>,
    seen: Option<StoredValue<Seen>>,
) -> Classes {
    let me = me();
    let st = store().status.get();
    let at = (names.as_ptr() as usize, names.len());
    let was = seen.and_then(|s| s.get_value());
    let changed = was.as_ref().and_then(|(old, was_me, was_at)| {
        (*was_me == me && *was_at == at)
            .then(|| live::changed(section, old.as_deref(), st.as_deref()))
            .flatten()
    });
    let reuse = live::Reuse { prev, changed };
    let ix = Index::new(section, st.as_deref(), &Value::Null, Files::default());
    let out = live::classes(section, names, &ix, me.as_deref(), &reuse);
    if let Some(seen) = seen {
        seen.set_value(Some((st, me, at)));
    }
    out
}

pub fn row_for(section: &str, name: &str) -> Option<Row> {
    let (row, names) = store().edit.draft.with(|d| {
        let draft = d.draft();
        let v = draft.get(section)?.get(name)?;
        let base_items = d.base().get(section).and_then(Value::as_object);
        let ix = Index::new(section, None, draft, Files::now());
        Some((
            one_row(section, name, v, base_items, &ix),
            vec![(name.to_string(), clients_for(section, v))],
        ))
    })?;
    let classes = classes_now(section, &names, None, None);
    Some(live::apply(row, classes.get(name)))
}

pub(super) fn base_for(section: &str) -> Base {
    store().edit.draft.with(|d| {
        let draft = d.draft();
        let base_items = d.base().get(section).and_then(Value::as_object);
        let items = draft.get(section).and_then(Value::as_object);
        let ix = Index::new(section, None, draft, Files::now());
        let mut rows: Vec<Row> = Vec::new();
        let mut names = Vec::new();
        for (name, v) in items.into_iter().flatten() {
            rows.push(one_row(section, name, v, base_items, &ix));
            names.push((name.clone(), clients_for(section, v)));
        }
        let gone = base_items.into_iter().flatten().filter(|(k, _)| {
            !items.is_some_and(|i| i.contains_key(*k))
                && !items.is_some_and(|i| {
                    i.keys()
                        .any(|n| d.renamed_from(section, n).as_deref() == Some(k.as_str()))
                })
        });
        for (name, _) in gone {
            rows.push(Row {
                key: name.clone(),
                to: Route::Detail(section.to_string(), name.clone()),
                icon: "trash",
                dot: None,
                title: name.clone(),
                meta: String::new(),
                line2: live::REMOVED.into(),
                tags: vec![("removed".into(), Tone::Danger)],
                group: String::new(),
                pending: None,
                filters: Vec::new(),
                actions: Vec::new(),
                ..Row::default()
            });
        }
        Base { rows, names }
    })
}

fn new_menu(section: &str, site: RwSignal<bool>, adding: RwSignal<bool>) -> Option<Vec<NewChoice>> {
    let pick = move |website: bool| {
        Callback::new(move |_| {
            site.set(website);
            adding.set(true);
        })
    };
    (section == ServiceEntry::section()).then(|| {
        vec![
            NewChoice {
                key: "service",
                label: "Service".into(),
                sub: "A TCP or UDP port".into(),
                on_pick: pick(false),
            },
            NewChoice {
                key: "website",
                label: "Website".into(),
                sub: "A name with HTTPS".into(),
                on_pick: pick(true),
            },
        ]
    })
}

type Filters = (Vec<(&'static str, String)>, Vec<&'static str>);

fn filters_for(section: &str) -> Filters {
    let (filters, groups): (Vec<(&'static str, String)>, Vec<&'static str>) =
        match entity_of_section(section) {
            Some(Entity::Client) => (
                vec![
                    ("all", "All".into()),
                    (
                        crate::screens::ONLINE_FILTER,
                        burrow_admin_logic::wording::capital(burrow_admin_logic::wording::ONLINE),
                    ),
                    (
                        crate::screens::OFFLINE_FILTER,
                        burrow_admin_logic::wording::capital(burrow_admin_logic::wording::OFFLINE),
                    ),
                ],
                vec!["Online", "Offline"],
            ),
            Some(Entity::Visitor) => (
                vec![
                    ("all", "All".into()),
                    ("admins", "Admins".into()),
                    ("active", "In use".into()),
                ],
                vec!["In use now", "Not in use"],
            ),
            _ => (
                vec![
                    ("all", "All".into()),
                    (route::PUBLIC_FILTER, "Public".into()),
                    (route::WEBSITES_FILTER, "Websites".into()),
                    (ServiceEntry::VISITORS.key(), "Visitors".into()),
                    (Protocol::Tcp.wire(), "TCP".into()),
                    (Protocol::Udp.wire(), "UDP".into()),
                ],
                vec!["Problems", "Up"],
            ),
        };
    (filters, groups)
}

fn list_rows(section: String) -> Memo<Vec<Row>> {
    live::provide_live_text(section.clone());
    let sec = section.clone();
    let base = Memo::new(move |_| base_for(&sec));
    let seen: StoredValue<Seen> = StoredValue::new(None);
    let classes = Memo::new(move |prev: Option<&Classes>| {
        base.with(|b| classes_now(&section, &b.names, prev, Some(seen)))
    });
    Memo::new(move |_| {
        base.with(|b| {
            classes.with(|c| {
                b.rows
                    .iter()
                    .map(|r| live::apply(r.clone(), c.get(&r.key)))
                    .collect::<Vec<_>>()
            })
        })
    })
}

#[component]
pub fn Entities(section: String) -> impl IntoView {
    let s = store();
    let sec = section.clone();
    let rows = list_rows(sec);
    let sec = section.clone();
    let selected = Memo::new(move |_| match s.route.get() {
        Route::Detail(x, name) if x == sec => Some(name),
        _ => None,
    });
    let sec = section.clone();
    let on_list = Memo::new(move |_| s.route.with(|r| matches!(r, Route::List(x) if *x == sec)));
    Effect::new(move |_| {
        if !on_list.get() || !wide() {
            return;
        }
        let asked = route::query(route::FILTER_QUERY);
        let first = rows.with_untracked(|r| {
            r.iter()
                .find(|r| {
                    r.icon != "trash" && asked.as_deref().is_none_or(|a| r.filters.contains(&a))
                })
                .map(|r| r.to.clone())
        });
        if let Some(to) = first {
            route::replace(to);
        }
    });
    let adding = RwSignal::new(false);
    let site = RwSignal::new(false);
    Effect::new(move |was: Option<Option<String>>| {
        let now = selected.get();
        if was.is_some_and(|w| w != now) {
            adding.set(false);
        }
        now
    });
    {
        let sec = section.clone();
        Effect::new(move |_| {
            if s.creating.get().as_deref() == Some(sec.as_str()) {
                s.creating.set(None);
                adding.set(true);
            }
        });
    }
    let what = singular(&section);
    let count = Signal::derive(move || {
        rows.with(|r| r.iter().filter(|x| x.icon != "trash").count())
            .to_string()
    });
    let (filters, groups) = filters_for(&section);
    let draft_name = RwSignal::new(String::new());
    let shown_rows = {
        let what = what.to_string();
        let section_new = section.clone();
        Signal::derive(move || {
            let mut r = rows.get();
            if adding.get() {
                let n = draft_name.get();
                r.insert(
                    0,
                    Row {
                        key: crate::screens::NEW_ROW.into(),
                        to: route::current(),
                        icon: crate::screens::entity_icon(&section_new),
                        dot: Some(Tone::Accent),
                        title: if site.get() && n.is_empty() {
                            "new website".to_string()
                        } else if n.is_empty() {
                            format!("new {what}")
                        } else {
                            n
                        },
                        meta: "new".into(),
                        line2: burrow_admin_logic::wording::NOT_APPLIED.into(),
                        ..Row::default()
                    },
                );
            }
            r
        })
    };
    let selected_sig = Signal::derive(move || {
        if adding.get() {
            Some(crate::screens::NEW_ROW.to_string())
        } else {
            selected.get()
        }
    });
    let can = s.can_write_now();
    let new_menu = new_menu(&section, site, adding);
    let kind = [
        ServiceEntry::section(),
        burrow_config::ClientEntry::section(),
        burrow_config::VisitorEntry::section(),
    ]
    .into_iter()
    .find(|k| *k == section);
    let list = view! {
        <ListPane
            title=plural(&section)
            count=count
            new_label=format!("New {what}")
            on_new=Callback::new(move |_| { site.set(false); adding.set(true) })
            new_menu=new_menu
            new_disabled=!can
            search_label=format!("Search {}", section)
            filters=filters
            groups=groups
            rows=shown_rows
            selected=selected_sig
            empty=(format!("No {} yet", section), format!("Add the first {what} with the + button."))
            testid="entity-list"
            by_file=kind
        />
    }
    .into_any();
    let pick = format!("Pick one of the {} on the left", section);
    let sec_new = section.clone();
    let detail = view! {
        {move || {
            let (sec, pick) = (sec_new.clone(), pick.clone());
            if adding.get() && site.get() {
                return view! { <DetailPane><NewWebsite open=adding draft_name=draft_name/></DetailPane> }.into_any();
            }
            if adding.get() {
                return view! { <DetailPane><NewEntity section=sec open=adding draft_name=draft_name/></DetailPane> }.into_any();
            }
            match selected.get() {
                Some(name) => view! { <Detail section=sec name=name/> }.into_any(),
                None => view! {
                    <div class="max-lg:hidden">
                        <DetailPane>
                            <EmptyState icon="stack" title=pick message="Or add a new one."/>
                        </DetailPane>
                    </div>
                }
                .into_any(),
            }
        }}
    }
    .into_any();
    master_detail(list, detail)
}
