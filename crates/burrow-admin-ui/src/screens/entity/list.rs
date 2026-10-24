use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_config::{ClientEntry, Entry, Protocol, ServiceEntry, VisitorEntry, entity_of_section};
use burrow_ui_kit::{EmptyState, Tone};
use leptos::prelude::*;

use crate::form::plain;
use crate::format;
use crate::route::{self, Route};
use crate::screens::create::{NewEntity, singular};
use crate::screens::paths::{clients_of, online};
use crate::screens::{DetailPane, ListPane, Row, master_detail, wide};
use crate::store::store;

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
            Some(Entity::Client) => {
                let c = status
                    .as_ref()
                    .and_then(|st| st.clients.iter().find(|c| &c.name == name))
                    .cloned();
                let on = c.as_ref().is_some_and(|c| c.connected);
                let served: Vec<String> = ServiceEntry::entries(&draft)
                    .map(|m| {
                        m.iter()
                            .filter(|(_, x)| clients_of(x).contains(name))
                            .map(|(n, _)| n.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let serves = if served.is_empty() {
                    "no services".to_string()
                } else {
                    served.join(", ")
                };
                let left = c.as_ref().and_then(|c| c.left_secs_ago);
                Row {
                    key: name.clone(),
                    to,
                    icon: "monitor",
                    dot: Some(if on { Tone::Success } else { Tone::Neutral }),
                    title: name.clone(),
                    meta: match (&c, on) {
                        (Some(c), true) => format!("{} open", c.open),
                        (Some(_), false) => left
                            .map(|a| format!("left {}", format::ago(a)))
                            .unwrap_or_else(|| "not seen".into()),
                        (None, _) => String::new(),
                    },
                    line2: if pending.is_some() {
                        "Draft · not applied yet".into()
                    } else if on {
                        let host = c
                            .as_ref()
                            .and_then(|c| c.from.clone())
                            .map(|f| f.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or(f))
                            .unwrap_or_default();
                        let since =
                            format::span(c.as_ref().and_then(|c| c.connected_secs).unwrap_or(0));
                        if host.is_empty() {
                            format!("for {since} · {serves}")
                        } else {
                            format!("{host} · for {since} · {serves}")
                        }
                    } else {
                        serves
                    },
                    group: if on {
                        "Online".into()
                    } else {
                        "Offline".into()
                    },
                    pending,
                    filters: vec![if on { "online" } else { "offline" }],
                    ..Row::default()
                }
            }
            Some(Entity::Visitor) => {
                let st = status
                    .as_ref()
                    .and_then(|st| st.visitors.iter().find(|c| &c.name == name))
                    .cloned();
                let open = st.as_ref().map(|x| x.open).unwrap_or(0);
                let seen = st.as_ref().and_then(|x| x.last_seen_secs);
                let in_use = open > 0 || me.as_deref() == Some(name.as_str());
                let admin = VisitorEntry::ADMIN.get(v).map(plain);
                let mut filters = vec![];
                if admin.is_some() {
                    filters.push("admins");
                }
                if in_use {
                    filters.push("active");
                }
                let may: Vec<String> = ServiceEntry::entries(&draft)
                    .map(|m| {
                        m.iter()
                            .filter(|(_, x)| {
                                ServiceEntry::VISITORS
                                    .get(x)
                                    .and_then(Value::as_array)
                                    .is_some_and(|a| {
                                        a.iter().any(|y| y.as_str() == Some(name.as_str()))
                                    })
                            })
                            .map(|(n, _)| n.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let reach: Vec<String> = VisitorEntry::REACH
                    .get(v)
                    .and_then(Value::as_object)
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default();
                let mut line2 = if may.is_empty() {
                    "no services".to_string()
                } else {
                    may.join(", ")
                };
                if !reach.is_empty() {
                    line2.push_str(&format!(" · reach: {}", reach.join(", ")));
                }
                Row {
                    key: name.clone(),
                    to,
                    icon: if admin.is_some() { "shield" } else { "person" },
                    dot: None,
                    title: name.clone(),
                    title_tag: admin.map(|a| {
                        (
                            a.clone(),
                            if a == "write" {
                                Tone::Danger
                            } else {
                                Tone::Accent
                            },
                        )
                    }),
                    meta: if me.as_deref() == Some(name.as_str()) && open == 0 {
                        "you".into()
                    } else if status.is_some() {
                        format::visitor_use_short(open, seen)
                    } else {
                        String::new()
                    },
                    line2,
                    tags: Vec::new(),
                    group: if in_use {
                        "In use now".into()
                    } else {
                        "Not in use".into()
                    },
                    pending,
                    filters,
                    ..Row::default()
                }
            }
            _ => {
                let st = status
                    .as_ref()
                    .and_then(|st| st.services.iter().find(|c| &c.name == name))
                    .cloned();
                let public = ServiceEntry::BIND.get(v).is_some_and(|b| !b.is_null());
                let protocol = ServiceEntry::PROTOCOL
                    .get(v)
                    .map(plain)
                    .unwrap_or_else(|| Protocol::default().wire().into());
                let udp = Protocol::from_wire(&protocol) == Some(Protocol::Udp);
                let cs = clients_of(v);
                let up = cs.iter().filter(|c| online(status.as_deref(), c)).count();
                let health_up = st
                    .as_ref()
                    .map(|x| (x.health.iter().filter(|h| h.up).count(), x.health.len()));
                let problem = status.is_some()
                    && (up < cs.len() || health_up.is_some_and(|(u, n)| n > 0 && u < n));
                let mut tags = vec![(
                    protocol.to_uppercase(),
                    if udp { Tone::Violet } else { Tone::Accent },
                )];
                if up == 0 && status.is_some() {
                    tags.push(("✕ Down".into(), Tone::Danger));
                } else if let Some((u, n)) = health_up.filter(|(u, n)| *n > 0 && u < n) {
                    tags.push((format!("⚠ {u} of {n} up"), Tone::Warning));
                } else if status.is_some() && up < cs.len() {
                    tags.push((format!("⚠ {up} of {} up", cs.len()), Tone::Warning));
                }
                Row {
                    key: name.clone(),
                    to,
                    icon: if public { "globe" } else { "lock" },
                    dot: Some(if !problem {
                        Tone::Success
                    } else if up == 0 {
                        Tone::Danger
                    } else {
                        Tone::Warning
                    }),
                    title: name.clone(),
                    meta: st
                        .as_ref()
                        .map(|x| format!("{} open", x.open))
                        .unwrap_or_default(),
                    line2: format!(
                        "{} → {}",
                        if public {
                            plain(ServiceEntry::BIND.get(v).unwrap_or(&Value::Null))
                        } else {
                            "private".into()
                        },
                        plain(ServiceEntry::TARGET.get(v).unwrap_or(&Value::Null))
                    ),
                    tags,
                    group: if problem {
                        "Problems".into()
                    } else {
                        "Up".into()
                    },
                    pending,
                    filters: vec![
                        if public { "public" } else { "private" },
                        if udp { Protocol::Udp } else { Protocol::Tcp }.wire(),
                    ],
                    actions: Vec::new(),
                    ..Row::default()
                }
            }
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

#[component]
pub fn Entities(section: String, selected: Option<String>) -> impl IntoView {
    let s = store();
    let sec = section.clone();
    let rows = Memo::new(move |_| rows_for(&sec));
    if selected.is_none()
        && wide()
        && let Some(first) = rows.get_untracked().into_iter().find(|r| r.icon != "trash")
    {
        route::replace(first.to);
    }
    let adding = RwSignal::new(false);
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
    let (filters, groups): (Vec<(&'static str, String)>, Vec<&'static str>) =
        match entity_of_section(&section) {
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
    let draft_name = RwSignal::new(String::new());
    let shown_rows = {
        let what = what.to_string();
        Signal::derive(move || {
            let mut r = rows.get();
            if adding.get() {
                let n = draft_name.get();
                r.insert(
                    0,
                    Row {
                        key: "__new".into(),
                        to: route::current(),
                        icon: if what == ClientEntry::ENTITY {
                            "monitor"
                        } else if what == VisitorEntry::ENTITY {
                            "person"
                        } else {
                            "stack"
                        },
                        dot: Some(Tone::Accent),
                        title: if n.is_empty() {
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
    let list = view! {
        <ListPane
            title=plural(&section)
            count=count
            new_label=format!("New {what}")
            on_new=Callback::new(move |_| adding.set(true))
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
