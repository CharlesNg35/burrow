use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_config::{Entry, Protocol, ServerConfig, ServiceEntry, WebConfig, entity_of_section};
use burrow_ui_kit::{EmptyState, Tone};
use leptos::prelude::*;

use crate::route::{self, Route};
use crate::screens::create::{NewEntity, NewWebsite, singular};
use crate::screens::{DetailPane, ListPane, NewChoice, Row, master_detail, wide};
use crate::store::store;

use super::rows::{client_row, service_row, visitor_row};
use super::{Detail, plural};

pub fn rows_for(section: &str) -> Vec<Row> {
    let s = store();
    let (draft, base) = s.edit.draft.with(|d| (d.draft().clone(), d.base().clone()));
    let status = s.status.get();
    let me = s
        .welcome
        .with(|w| w.as_ref().map(|w| w.you.visitor.clone()));
    let items = draft
        .get(section)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let base_items = base
        .get(section)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let https_on = ServerConfig::WEB
        .get(&draft)
        .and_then(|w| WebConfig::HTTPS.get(w))
        .is_some_and(|h| !h.is_null());
    let mut out = Vec::new();
    for (name, v) in &items {
        let pending = (!base_items.contains_key(name)
            && s.edit
                .draft
                .with(|d| d.renamed_from(section, name).is_none()))
        .then_some("new");
        let entity_path = vec![section.to_string(), name.clone()];
        let mark = if s.edit.unsaved(&entity_path) {
            Some(("unsaved".to_string(), Tone::Warning))
        } else if pending.is_none() && base_items.get(name) != Some(v) {
            Some(("changed".to_string(), Tone::Accent))
        } else {
            None
        };
        let to = Route::Detail(section.to_string(), name.clone());
        let row = match entity_of_section(section) {
            Some(Entity::Client) => client_row(name, &draft, status.as_deref(), pending, to),
            Some(Entity::Visitor) => visitor_row(
                name,
                v,
                &draft,
                status.as_deref(),
                me.as_deref(),
                pending,
                to,
            ),
            _ => service_row(name, (v, https_on), status.as_deref(), pending, to),
        };
        let mut row = row;
        if let Some(m) = mark {
            row.tags.insert(0, m);
        }
        out.push(row);
    }
    let renamed: Vec<String> = s.edit.draft.with(|d| {
        items
            .keys()
            .filter_map(|n| d.renamed_from(section, n))
            .collect()
    });
    for name in base_items
        .keys()
        .filter(|k| !items.contains_key(*k) && !renamed.contains(k))
    {
        out.push(Row {
            key: name.clone(),
            to: Route::Detail(section.to_string(), name.clone()),
            icon: "trash",
            dot: None,
            title: name.clone(),
            meta: String::new(),
            line2: "removed in your changes".into(),
            tags: vec![("removed".into(), Tone::Danger)],
            group: String::new(),
            pending: None,
            filters: Vec::new(),
            actions: Vec::new(),
            ..Row::default()
        });
    }
    out
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

fn filters_for(section: &str, rows: Memo<Vec<Row>>) -> Filters {
    let s = store();
    let (mut filters, groups): (Vec<(&'static str, String)>, Vec<&'static str>) =
        match entity_of_section(section) {
            Some(Entity::Client) => (
                vec![
                    ("all", "All".into()),
                    ("online", "Online".into()),
                    ("offline", "Offline".into()),
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
                    ("public", "Public".into()),
                    ("private", "Private".into()),
                    (Protocol::Tcp.wire(), "TCP".into()),
                    (Protocol::Udp.wire(), "UDP".into()),
                ],
                vec!["Problems", "Up"],
            ),
        };
    let sites = rows.with_untracked(|r| {
        r.iter()
            .any(|x| x.filters.contains(&route::WEBSITES_FILTER))
    });
    if sites
        || s.edit.draft.with_untracked(|d| {
            ServerConfig::WEB
                .get(d.draft())
                .is_some_and(|w| !w.is_null())
        })
    {
        filters.push((route::WEBSITES_FILTER, "Websites".into()));
    }
    (filters, groups)
}

#[component]
pub fn Entities(section: String, selected: Option<String>) -> impl IntoView {
    let s = store();
    let sec = section.clone();
    let rows = Memo::new(move |_| rows_for(&sec));
    if selected.is_none()
        && wide()
        && let Some(first) = {
            let asked = route::query(route::FILTER_QUERY);
            rows.get_untracked().into_iter().find(|r| {
                r.icon != "trash" && asked.as_deref().is_none_or(|a| r.filters.contains(&a))
            })
        }
    {
        route::replace(first.to);
    }
    let adding = RwSignal::new(false);
    let site = RwSignal::new(false);
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
    let (filters, groups) = filters_for(&section, rows);
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
                        key: "__new".into(),
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
                        line2: "Draft · not applied yet".into(),
                        ..Row::default()
                    },
                );
            }
            r
        })
    };
    let sel = selected.clone();
    let selected_sig = Signal::derive(move || {
        if adding.get() {
            Some("__new".to_string())
        } else {
            sel.clone()
        }
    });
    let can = s.can_write_now();
    let new_menu = new_menu(&section, site, adding);
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
        />
    }
    .into_any();
    let pick = format!("Pick one of the {} on the left", section);
    let sec_new = section.clone();
    let sel_name = selected.clone();
    let detail = view! {
        {move || {
            let (sec, pick) = (sec_new.clone(), pick.clone());
            if adding.get() && site.get() {
                return view! { <DetailPane><NewWebsite open=adding draft_name=draft_name/></DetailPane> }.into_any();
            }
            if adding.get() {
                return view! { <DetailPane><NewEntity section=sec open=adding draft_name=draft_name/></DetailPane> }.into_any();
            }
            match sel_name.clone() {
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
