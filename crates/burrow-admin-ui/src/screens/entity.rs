use burrow_admin_logic::changeset::{get, ops_between};
use burrow_admin_logic::rates::{human_bits, human_bytes};
use burrow_admin_logic::tree::{entity_of, referrers};
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::value::as_secret;
use burrow_ui_kit::{
    Button, Callout, Card, EmptyState, Icon, PathDiagram, Stat, Stats, StatusDot, Table, Tabs,
    Tone, button_class, copy_text, tag_class,
};
use leptos::context::Provider;
use leptos::prelude::*;

use crate::form::{EditMode, object_view, plain};
use crate::format;
use crate::route::{self, Link, Route};
use crate::screens::create::{NewEntity, RenameEntity, singular};
use crate::screens::paths::{self, Problems, clients_of, online};
use crate::screens::{DetailPane, ListPane, Row, master_detail, wide};
use crate::store::store;

fn entity(section: &str) -> Entity {
    match section {
        "clients" => Entity::Client,
        "visitors" => Entity::Visitor,
        _ => Entity::Service,
    }
}

fn plural(section: &str) -> &'static str {
    match section {
        "clients" => "Clients",
        "visitors" => "Visitors",
        _ => "Services",
    }
}

fn secrets(v: &Value) -> usize {
    match v.get("secret") {
        Some(Value::Array(a)) => a.len(),
        Some(x) if as_secret(x).is_some() || x.is_object() => 1,
        _ => 0,
    }
}

fn rows_for(section: &str) -> Vec<Row> {
    let s = store();
    let (draft, base) = s.edit.draft.with(|d| (d.draft().clone(), d.base().clone()));
    let status = s.status.get();
    let rates = s.rates.get();
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
        let pending = (!base_items.contains_key(name)).then_some("new");
        let to = Route::Detail(section.to_string(), name.clone());
        let row = match section {
            "clients" => {
                let c = status
                    .as_ref()
                    .and_then(|st| st.clients.iter().find(|c| &c.name == name))
                    .cloned();
                let on = c.as_ref().is_some_and(|c| c.connected);
                let served: Vec<String> = draft
                    .get("services")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .filter(|(_, x)| clients_of(x).contains(name))
                            .map(|(n, _)| n.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let since = c
                    .as_ref()
                    .map(|c| match (c.connected_secs, c.left_secs_ago) {
                        (Some(x), _) => format!("for {}", format::span(x)),
                        (None, left) => {
                            format::left(left, status.as_ref().map(|s| s.uptime_secs).unwrap_or(0))
                        }
                    });
                Row {
                    key: name.clone(),
                    to,
                    icon: "monitor",
                    dot: Some(if on { Tone::Success } else { Tone::Neutral }),
                    title: name.clone(),
                    meta: c
                        .as_ref()
                        .map(|c| format!("{} open", c.open))
                        .unwrap_or_default(),
                    line2: if on {
                        format!(
                            "{} · {}",
                            c.as_ref().and_then(|c| c.from.clone()).unwrap_or_default(),
                            since.unwrap_or_default()
                        )
                    } else {
                        since.unwrap_or_else(|| "offline".into())
                    },
                    tags: if !on && status.is_some() {
                        vec![(
                            format::away_badge(c.as_ref().and_then(|c| c.left_secs_ago))
                                .to_string(),
                            Tone::Neutral,
                        )]
                    } else {
                        served
                            .iter()
                            .take(3)
                            .map(|n| (n.clone(), Tone::Neutral))
                            .collect()
                    },
                    group: if on {
                        "Online".into()
                    } else {
                        "Offline".into()
                    },
                    pending,
                    filters: vec![if on { "online" } else { "offline" }],
                    actions: Vec::new(),
                }
            }
            "visitors" => {
                let st = status
                    .as_ref()
                    .and_then(|st| st.visitors.iter().find(|c| &c.name == name))
                    .cloned();
                let open = st.as_ref().map(|x| x.open).unwrap_or(0);
                let admin = v.get("admin").map(plain);
                let mut tags = Vec::new();
                if open > 0 {
                    tags.push(("active".to_string(), Tone::Success));
                }
                if let Some(a) = &admin {
                    tags.push((
                        format!("admin · {a}"),
                        if a == "write" {
                            Tone::Danger
                        } else {
                            Tone::Accent
                        },
                    ));
                }
                let mut filters = vec![];
                if admin.is_some() {
                    filters.push("admins");
                }
                if open > 0 {
                    filters.push("active");
                }
                Row {
                    key: name.clone(),
                    to,
                    icon: if admin.is_some() { "shield" } else { "person" },
                    dot: None,
                    title: name.clone(),
                    meta: format!("{open} open"),
                    line2: format!(
                        "{} · {} total",
                        admin
                            .map(|a| format!("admin · {a}"))
                            .unwrap_or_else(|| "visitor".into()),
                        st.as_ref().map(|x| x.total).unwrap_or(0)
                    ),
                    tags,
                    group: if open > 0 {
                        "Active".into()
                    } else {
                        "Not active".into()
                    },
                    pending,
                    filters,
                    actions: Vec::new(),
                }
            }
            _ => {
                let st = status
                    .as_ref()
                    .and_then(|st| st.services.iter().find(|c| &c.name == name))
                    .cloned();
                let public = v.get("bind").is_some_and(|b| !b.is_null());
                let protocol = v.get("protocol").map(plain).unwrap_or_else(|| "tcp".into());
                let cs = clients_of(v);
                let up = cs.iter().filter(|c| online(status.as_deref(), c)).count();
                let health_up = st
                    .as_ref()
                    .map(|x| (x.health.iter().filter(|h| h.up).count(), x.health.len()));
                let problem =
                    status.is_some() && (up == 0 || health_up.is_some_and(|(u, n)| n > 0 && u < n));
                let mut tags = vec![(
                    protocol.to_uppercase(),
                    if protocol == "udp" {
                        Tone::Udp
                    } else {
                        Tone::Accent
                    },
                )];
                if up == 0 && status.is_some() {
                    tags.push(("Down".into(), Tone::Danger));
                } else if let Some((u, n)) = health_up.filter(|(u, n)| *n > 0 && u < n) {
                    tags.push((format!("{u} of {n} up"), Tone::Warning));
                }
                if let Some(x) = &st {
                    tags.push((format!("{} total", format::count(x.total)), Tone::Neutral));
                }
                let r = rates.service(name);
                let _ = r;
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
                        "{} · {} → {}",
                        protocol.to_uppercase(),
                        if public {
                            plain(v.get("bind").unwrap_or(&Value::Null))
                        } else {
                            "private".into()
                        },
                        plain(v.get("target").unwrap_or(&Value::Null))
                    ),
                    tags,
                    group: if problem {
                        "Problems".into()
                    } else {
                        "Healthy".into()
                    },
                    pending,
                    filters: vec![
                        if public { "public" } else { "private" },
                        if protocol == "udp" { "udp" } else { "tcp" },
                    ],
                    actions: Vec::new(),
                }
            }
        };
        out.push(row);
    }
    for (name, v) in base_items.iter().filter(|(k, _)| !items.contains_key(*k)) {
        let _ = v;
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
    let what = singular(&section);
    let sec2 = section.clone();
    let count = Signal::derive(move || {
        let r = rows.get();
        let live: Vec<&Row> = r.iter().filter(|x| x.icon != "trash").collect();
        if sec2 == "clients" {
            format!(
                "{}/{}",
                live.iter().filter(|x| x.group == "Online").count(),
                live.len()
            )
        } else {
            live.len().to_string()
        }
    });
    let (filters, groups): (Vec<(&'static str, String)>, Vec<&'static str>) = match section.as_str()
    {
        "clients" => (
            vec![
                ("all", "All".into()),
                ("online", "Online".into()),
                ("offline", "Offline".into()),
            ],
            vec!["Online", "Offline"],
        ),
        "visitors" => (
            vec![
                ("all", "All".into()),
                ("admins", "Admins".into()),
                ("active", "Active".into()),
            ],
            vec!["Active", "Not active"],
        ),
        _ => (
            vec![
                ("all", "All".into()),
                ("public", "Public".into()),
                ("private", "Private".into()),
                ("tcp", "TCP".into()),
                ("udp", "UDP".into()),
            ],
            vec!["Problems", "Healthy"],
        ),
    };
    let sel = selected.clone();
    let list = view! {
        <ListPane
            title=plural(&section)
            count=count
            new_label=format!("New {what}")
            on_new=Callback::new(move |_| adding.set(true))
            search_label=format!("Search {}", section)
            filters=filters
            groups=groups
            rows=rows
            selected=Signal::stored(sel)
            empty=(format!("No {} yet", section), format!("Add the first {what} with the + button."))
            testid="entity-list"
        />
    }
    .into_any();
    let list = if s.can_write_now() {
        list
    } else {
        let sel = selected.clone();
        view! {
            <ListPane
                title=plural(&section)
                count=count
                search_label=format!("Search {}", section)
                filters=match section.as_str() {
                    "clients" => vec![("all", "All".into()), ("online", "Online".into()), ("offline", "Offline".into())],
                    "visitors" => vec![("all", "All".into()), ("admins", "Admins".into()), ("active", "Active".into())],
                    _ => vec![("all", "All".into()), ("public", "Public".into()), ("private", "Private".into()), ("tcp", "TCP".into()), ("udp", "UDP".into())],
                }
                groups=match section.as_str() {
                    "clients" => vec!["Online", "Offline"],
                    "visitors" => vec!["Active", "Not active"],
                    _ => vec!["Problems", "Healthy"],
                }
                rows=rows
                selected=Signal::stored(sel)
                testid="entity-list"
            />
        }
        .into_any()
    };
    let pick = format!("Pick one of the {} on the left", section);
    let sec_new = section.clone();
    let detail = match selected {
        Some(name) => view! { <Detail section=section.clone() name=name/> }.into_any(),
        None => view! {
            <DetailPane>
                <EmptyState icon="services" title=pick message="Or add a new one."/>
            </DetailPane>
        }
        .into_any(),
    };
    view! {
        <NewEntity section=sec_new open=adding/>
        {master_detail(list, detail)}
    }
}

fn problems_for(path: &Path) -> Problems {
    let e = store().edit;
    let errs: Vec<(Path, String)> = e
        .related
        .with(|r| {
            r.iter()
                .map(|(p, m)| (p.clone(), m.clone()))
                .collect::<Vec<_>>()
        })
        .into_iter()
        .chain(e.local.with(|l| {
            l.iter()
                .map(|(p, m)| (p.clone(), m.clone()))
                .collect::<Vec<_>>()
        }))
        .chain(e.plan.with(|p| {
            p.as_ref()
                .map(|p| {
                    p.diags
                        .iter()
                        .filter(|d| d.is_error())
                        .filter_map(|d| d.path.clone().map(|x| (x, d.message.clone())))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        }))
        .filter(|(p, _)| p.starts_with(path))
        .collect();
    let at = |k: &str| {
        errs.iter()
            .find(|(p, _)| p.get(2).map(String::as_str) == Some(k))
            .map(|(_, m)| m.clone())
    };
    let doc = e.doc();
    let ports = at("target").map(|_| {
        let b = paths::port_count(get(
            &doc,
            &[path[0].clone(), path[1].clone(), "bind".into()],
        ));
        let t = paths::port_count(get(
            &doc,
            &[path[0].clone(), path[1].clone(), "target".into()],
        ));
        format!("{t} ≠ {b} ports")
    });
    Problems {
        source: at("allow_from")
            .or_else(|| at("deny_from"))
            .map(|_| "invalid CIDR".into()),
        clients: at("health_check").map(|_| "health".into()),
        ports,
    }
}

#[component]
fn Detail(section: String, name: String) -> impl IntoView {
    let s = store();
    let e = s.edit;
    let path: Path = vec![section.clone(), name.clone()];
    let exists = {
        let path = path.clone();
        Memo::new(move |_| e.draft.with(|d| get(d.draft(), &path).is_some()))
    };
    let fresh = e
        .draft
        .with_untracked(|d| get(d.base(), &path).is_none() && get(d.draft(), &path).is_some());
    let editing = RwSignal::new(fresh && s.can_write_now());
    let snapshot: RwSignal<Option<Value>> = RwSignal::new(None);
    let mode = EditMode(editing.into());
    let tab: RwSignal<&'static str> = RwSignal::new(match route::query("tab").as_deref() {
        Some("settings") => "settings",
        Some("access") => "access",
        Some("secrets") => "secrets",
        Some("services") => "services",
        Some("connect") => "connect",
        _ if fresh => "settings",
        _ => "overview",
    });
    Effect::new(move |first: Option<()>| {
        let t = tab.get();
        if first.is_some() {
            route::set_query("tab", t);
        }
    });
    let renaming = RwSignal::new(false);
    let menu = RwSignal::new(false);
    let what = singular(&section);
    if section == "clients"
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
            snapshot.set(e.peek(&path));
            editing.set(true);
            if tab.get_untracked() == "overview" {
                tab.set("settings");
            }
        }
    };
    let changed = {
        let path = path.clone();
        Signal::derive(move || {
            let now = e.value(&path).unwrap_or(Value::Null);
            snapshot.with(|b| ops_between(b.as_ref().unwrap_or(&Value::Null), &now).len())
        })
    };
    let problems = {
        let path = path.clone();
        Signal::derive(move || {
            let mut out: Vec<String> = e.related.with(|r| {
                r.iter()
                    .filter(|(p, _)| p.starts_with(&path))
                    .map(|(p, _)| p.last().cloned().unwrap_or_default())
                    .collect()
            });
            out.extend(e.local.with(|l| {
                l.iter()
                    .filter(|(p, _)| p.starts_with(&path))
                    .map(|(p, _)| p.last().cloned().unwrap_or_default())
                    .collect::<Vec<_>>()
            }));
            out.extend(e.plan.with(|p| {
                p.as_ref()
                    .map(|p| {
                        p.diags
                            .iter()
                            .filter(|d| {
                                d.is_error()
                                    && d.path.as_ref().is_some_and(|x| x.starts_with(&path))
                            })
                            .map(|d| {
                                d.path
                                    .as_ref()
                                    .and_then(|x| x.last().cloned())
                                    .unwrap_or_default()
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            }));
            out.sort();
            out.dedup();
            out
        })
    };
    let discard = {
        let path = path.clone();
        move || {
            let snap = snapshot.get_untracked();
            e.set(&path, snap);
            editing.set(false);
        }
    };
    let save = move || {
        editing.set(false);
        s.toast(Tone::Success, "Added to changes", Some("Review"));
    };
    let (sec, nm) = (section.clone(), name.clone());
    let path_data = Signal::derive(move || {
        let doc = e.doc();
        let status = s.status.get();
        match sec.as_str() {
            "clients" => {
                let listen = status
                    .as_ref()
                    .and_then(|st| st.listening.first().map(|l| format::listen_addr(l)))
                    .unwrap_or_default();
                paths::client(&nm, &doc, status.as_deref(), &listen)
            }
            "visitors" => paths::visitor(&nm, &doc, status.as_deref()),
            _ => {
                let svc = get(&doc, &[sec.clone(), nm.clone()])
                    .cloned()
                    .unwrap_or(Value::Null);
                let rate = s.rates.with(|r| r.service(&nm).bytes_in);
                paths::service(
                    &nm,
                    &svc,
                    status.as_deref(),
                    rate,
                    &problems_for(&vec![sec.clone(), nm.clone()]),
                    editing.get(),
                )
            }
        }
    });
    let summary = {
        let (sec, nm) = (section.clone(), name.clone());
        move || {
            let doc = e.doc();
            let st = s.status.get();
            let edit = if editing.get() { "Editing · " } else { "" };
            match sec.as_str() {
                "clients" => {
                    let c = st
                        .as_ref()
                        .and_then(|x| x.clients.iter().find(|c| c.name == nm))
                        .cloned();
                    format!(
                        "{edit}{}",
                        match c {
                            Some(c) if c.connected => format!(
                                "online from {} · {} open",
                                c.from.unwrap_or_default(),
                                c.open
                            ),
                            Some(_) => "offline".into(),
                            None => "status not known yet".into(),
                        }
                    )
                }
                "visitors" => {
                    let v = st
                        .as_ref()
                        .and_then(|x| x.visitors.iter().find(|c| c.name == nm))
                        .cloned();
                    format!(
                        "{edit}{} open · {} total",
                        v.as_ref().map(|v| v.open).unwrap_or(0),
                        v.as_ref().map(|v| v.total).unwrap_or(0)
                    )
                }
                _ => {
                    let svc = get(&doc, &[sec.clone(), nm.clone()])
                        .cloned()
                        .unwrap_or(Value::Null);
                    let cs = clients_of(&svc);
                    let x = st
                        .as_ref()
                        .and_then(|x| x.services.iter().find(|c| c.name == nm))
                        .cloned();
                    let how = if cs.len() > 1 {
                        format!(
                            "{} over {} clients",
                            if svc.get("mode").map(plain).as_deref() == Some("balance") {
                                "Balanced"
                            } else {
                                "Failover"
                            },
                            cs.len()
                        )
                    } else {
                        format!(
                            "On {}",
                            cs.first().cloned().unwrap_or_else(|| "no client".into())
                        )
                    };
                    format!(
                        "{edit}{how} · {} open · {} total",
                        x.as_ref().map(|x| x.open).unwrap_or(0),
                        format::count(x.as_ref().map(|x| x.total).unwrap_or(0))
                    )
                }
            }
        }
    };
    let protocol_tag = {
        let (sec, nm) = (section.clone(), name.clone());
        move || {
            (sec == "services").then(|| {
                let p = e
                    .doc()
                    .get("services")
                    .and_then(|m| m.get(&nm))
                    .and_then(|v| v.get("protocol"))
                    .map(plain)
                    .unwrap_or_else(|| "tcp".into());
                let tone = if p == "udp" { Tone::Udp } else { Tone::Accent };
                view! { <span class=tag_class(tone)>{p.to_uppercase()}</span> }
            })
        }
    };
    let tabs: Vec<(&'static str, String)> = match section.as_str() {
        "clients" => vec![
            ("overview", "Overview".into()),
            ("secrets", "Secrets".into()),
            ("services", "Services".into()),
            ("connect", "Connect".into()),
            ("settings", "Settings".into()),
        ],
        "visitors" => vec![
            ("overview", "Overview".into()),
            ("access", "Access".into()),
            ("secrets", "Secrets".into()),
            ("connect", "Connect".into()),
            ("settings", "Settings".into()),
        ],
        _ => vec![
            ("overview", "Overview".into()),
            ("settings", "Settings".into()),
            ("access", "Access".into()),
        ],
    };
    let can = s.can_write_now();
    let head_actions = {
        let start_edit = start_edit.clone();
        let (path, sec, nm) = (path.clone(), section.clone(), name.clone());
        move || {
            if editing.get() {
                return view! { <span class=tag_class(Tone::Accent) data-testid="editing-tag"><Icon name="edit" small=true/>"Editing"</span> }.into_any();
            }
            let start_edit = start_edit.clone();
            let (path, sec, nm) = (path.clone(), sec.clone(), nm.clone());
            let back = Route::List(sec.clone());
            view! {
                <Button
                    label="Edit"
                    icon="edit"
                    testid="edit"
                    disabled=!can
                    reason=if can { String::new() } else { "Editing needs write admin.".to_string() }
                    on_click=move |_| start_edit()
                />
                <div class="relative">
                    <button type="button" class=button_class("secondary", false) aria-label="More actions" aria-haspopup="true" aria-expanded=move || menu.get().to_string() data-testid="more" on:click=move |_| menu.update(|m| *m = !*m)>
                        <Icon name="more"/>
                    </button>
                    <Show when=move || menu.get()>
                        {
                            let (path, nm, back, sec) = (path.clone(), nm.clone(), back.clone(), sec.clone());
                            view! {
                                <div class="absolute top-10 right-0 z-30 w-48 rounded-[10px] border border-border-strong bg-surface p-1 shadow-overlay dark:bg-surface-2" on:click=move |_| menu.set(false)>
                                    {can.then(|| view! { <button type="button" class="block w-full cursor-pointer rounded-md px-2 py-1.5 text-left text-sm hover:bg-surface-3" data-testid="rename-open" on:click=move |_| renaming.set(true)>"Rename…"</button> })}
                                    {can.then(|| {
                                        let (path, back) = (path.clone(), back.clone());
                                        view! { <button type="button" class="block w-full cursor-pointer rounded-md px-2 py-1.5 text-left text-sm text-danger hover:bg-surface-3" data-testid="delete" on:click=move |_| {
                                            e.set(&path, None);
                                            route::go(back.clone());
                                        }>{format!("Delete {}…", singular(&sec))}</button> }
                                    })}
                                    {(sec != "services").then(|| view! { <button type="button" class="block w-full cursor-pointer rounded-md px-2 py-1.5 text-left text-sm hover:bg-surface-3" data-testid="connect-open" on:click=move |_| tab.set("connect")>"Connect"</button> })}
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
                        <EmptyState icon="trash" title=format!("No {} named {nm}", singular(&sec)) message="It may have been removed or renamed." testid="detail-missing">
                            <Link to=back class=button_class("secondary", false)>{format!("Back to {}", plural(&sec))}</Link>
                        </EmptyState>
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
                    (_, "overview") => { let summary = object_view(&node, path2.clone(), spath.clone(), path2.clone()); view! {
                        {overview(&sec2, &nm2, false)}
                        <Card title="Settings" testid="settings-summary">
                            {summary}
                        </Card>
                    }
                    .into_any() }
                    ("clients", "secrets") | ("visitors", "secrets") => {
                        let p = vec![sec2.clone(), nm2.clone(), "secret".into()];
                        crate::secrets::secret_list(p, "secrets".into(), 4)
                    }
                    ("clients", "services") => served(&nm2).into_any(),
                    (_, "connect") => view! { <crate::secrets::ConnectPanel section=sec2.clone() name=nm2.clone()/> }.into_any(),
                    (_, "access") => {
                        let keep: &[&str] = if sec2 == "visitors" { &["reach", "max_connections", "admin", "admin_from"] } else { &["visitors", "allow_from", "deny_from"] };
                        let skip: Vec<&str> = burrow_admin_logic::tree::fields(&node).iter().map(|f| f.key.as_str()).filter(|k| !keep.contains(k)).collect();
                        crate::form::object_view_skipping(&node, path2.clone(), spath.clone(), path2.clone(), &skip)
                    }
                    _ => object_view(&node, path2.clone(), spath.clone(), path2.clone()),
                }
            };
            let nm_p = nm.clone();
            Some(view! {
                <PathDiagram data=path_data testid="path"/>
                {overview(&sec, &nm, true)}
                <Tabs tabs=tabs.clone() active=tab/>
                <div data-testid="detail-settings">
                    {move || (editing.get() && !problems.get().is_empty()).then(|| {
                        let p = problems.get();
                        view! {
                            <div class="mb-4" data-testid="form-problems">
                                <Callout tone=Tone::Danger title=format!("{} problem{} in {} {}", p.len(), if p.len() == 1 { "" } else { "s" }, what, nm_p)>
                                    <p>"Fix them to save. The path above marks where they are."</p>
                                </Callout>
                            </div>
                        }
                    })}
                    {tab_body}
                </div>
                {related(&sec, &nm)}
            }
            .into_any())
        }
    };
    let save_bar = {
        let discard = discard.clone();
        move || {
            editing.get().then(|| {
                let discard = discard.clone();
                let p = problems.get();
                let bad = !p.is_empty();
                view! {
                    <div class=format!("sticky bottom-4 z-20 mt-6 flex items-center gap-3 rounded-card border bg-surface px-4 py-3 shadow-overlay {}", if bad { "border-danger" } else { "border-border" }) data-testid="save-bar" data-sticky-bar="">
                        {if bad {
                            view! { <span class="flex items-center gap-1.5 text-sm font-semibold text-danger"><Icon name="alert-circle"/>{format!("{} problem{}", p.len(), if p.len() == 1 { "" } else { "s" })}<span class="font-normal text-text-muted">{format!(" · {}", p.join(", "))}</span></span> }.into_any()
                        } else {
                            view! { <span class="text-sm text-text-muted" data-testid="fields-changed">{move || { let n = changed.get(); format!("{n} field{} changed", if n == 1 { "" } else { "s" }) }}</span> }.into_any()
                        }}
                        <span class="ml-auto"></span>
                        <Button label="Discard" variant="ghost" testid="discard-edit" on_click=move |_| discard()/>
                        <Button label="Save to changes" variant="primary" testid="save" disabled=bad reason=if bad { "Fix the problems first".to_string() } else { String::new() } on_click=move |_| save()/>
                    </div>
                }
            })
        }
    };
    let crumb = Route::List(section.clone());
    let (sec_r, name_r) = (section.clone(), name.clone());
    let (sec_d, name_d, plural_s) = (section.clone(), name.clone(), plural(&section));
    view! {
        <Provider value=mode>
        <DetailPane back=crumb.clone()>
            <nav class="mb-1.5 flex gap-1.5 text-sm text-text-muted" aria-label="Breadcrumb">
                <Link to=crumb class="text-text-muted no-underline hover:text-text">{plural_s}</Link>
                <span aria-hidden="true">"/"</span>
                <span class="font-semibold text-text">{name.clone()}</span>
                {move || editing.get().then(|| view! { <span aria-hidden="true">"/"</span><span class="font-semibold text-text">"Edit"</span> })}
            </nav>
            <header class="mb-[22px] flex items-start gap-3.5">
                <div class="min-w-0 flex-1">
                    <h1 class="flex items-center gap-2.5 text-[25px] font-bold tracking-[-0.015em]">{name.clone()}{protocol_tag}</h1>
                    <p class="mt-1 text-base text-text-muted" data-testid="summary">{summary}</p>
                </div>
                <div class="flex items-center gap-2">{head_actions}</div>
            </header>
            <RenameEntity section=sec_r name=name_r open=renaming/>
            <div data-testid="detail" data-entity=sec_d data-name=name_d>{body}</div>
            {save_bar}
        </DetailPane>
        </Provider>
    }
}

fn overview(section: &str, name: &str, stats: bool) -> impl IntoView {
    let s = store();
    let (section, name) = (section.to_string(), name.to_string());
    move || {
        let (section, name) = (section.clone(), name.clone());
        let st = s.status.get();
        let doc = s.edit.doc();
        match section.as_str() {
            "clients" => {
                let c = st
                    .as_ref()
                    .and_then(|x| x.clients.iter().find(|c| c.name == name))
                    .cloned();
                let n = doc
                    .get("clients")
                    .and_then(|m| m.get(&name))
                    .map(secrets)
                    .unwrap_or(0);
                if !stats {
                    return view! { <div class="mb-4">{served(&name)}</div> }.into_any();
                }
                view! {
                    <Stats testid="live">
                        <Stat label="Open" value=c.as_ref().map(|c| format::count(c.open)).unwrap_or_default()/>
                        <Stat label="Idle channels" value=c.as_ref().map(|c| c.idle_channels.to_string()).unwrap_or_default()/>
                        <Stat label="Since" value=c.as_ref().map(|c| match (c.connected_secs, c.left_secs_ago) {
                            (Some(x), _) => format::span(x),
                            (None, Some(a)) => format!("left {}", format::ago(a)),
                            _ => "—".into(),
                        }).unwrap_or_default() detail=c.as_ref().and_then(|c| c.from.clone()).unwrap_or_default()/>
                        <Stat label="Secrets" value=n.to_string()/>
                    </Stats>
                }
                .into_any()
            }
            "visitors" => {
                let v = st
                    .as_ref()
                    .and_then(|x| x.visitors.iter().find(|c| c.name == name))
                    .cloned();
                if !stats {
                    return ().into_any();
                }
                view! {
                    <Stats testid="live">
                        <Stat label="Open visits" value=v.as_ref().map(|v| format::count(v.open)).unwrap_or_default()/>
                        <Stat label="Total visits" value=v.as_ref().map(|v| format::count(v.total)).unwrap_or_default()/>
                        <Stat label="Admin" value=doc.get("visitors").and_then(|m| m.get(&name)).and_then(|v| v.get("admin")).map(plain).unwrap_or_else(|| "none".into())/>
                        <Stat label="Secrets" value=doc.get("visitors").and_then(|m| m.get(&name)).map(secrets).unwrap_or(0).to_string()/>
                    </Stats>
                }
                .into_any()
            }
            _ => {
                let x = st
                    .as_ref()
                    .and_then(|x| x.services.iter().find(|c| c.name == name))
                    .cloned();
                let r = s.rates.get().service(&name);
                let (i, iu) = human_bits(r.bytes_in);
                let (o, ou) = human_bits(r.bytes_out);
                let svc = doc
                    .get("services")
                    .and_then(|m| m.get(&name))
                    .cloned()
                    .unwrap_or(Value::Null);
                let health = x.as_ref().map(|x| x.health.clone()).unwrap_or_default();
                let aside = x.as_ref().map(|x| x.set_aside.clone()).unwrap_or_default();
                let rows = clients_of(&svc)
                    .into_iter()
                    .map(|c| {
                        let cs = st.as_ref().and_then(|x| x.clients.iter().find(|y| y.name == c)).cloned();
                        let on = cs.as_ref().is_some_and(|y| y.connected);
                        let h = health.iter().find(|h| h.client == c).map(|h| h.up);
                        let set = aside.iter().find(|a| a.client == c).map(|a| if a.refused { "set aside (refused)" } else { "set aside" }).unwrap_or("not set aside");
                        let label = c.clone();
                        view! {
                            <tr data-row=c.clone()>
                                <th scope="row"><StatusDot on=on text=label/></th>
                                <td class="font-mono text-text-muted">{cs.and_then(|y| y.from).unwrap_or_else(|| "offline".into())}</td>
                                <td>{match h {
                                    Some(true) => view! { <span class=tag_class(Tone::Success)>"up"</span> }.into_any(),
                                    Some(false) => view! { <span class=tag_class(Tone::Danger)>"down"</span> }.into_any(),
                                    None => view! { <span class="text-text-faint">"not checked"</span> }.into_any(),
                                }}</td>
                                <td>{set}</td>
                            </tr>
                        }
                    })
                    .collect_view();
                if stats {
                    return view! {
                        <Stats testid="live">
                            <Stat label="Open" value=x.as_ref().map(|x| format::count(x.open)).unwrap_or_else(|| "0".into()) detail=x.as_ref().map(|x| format!("{} total", format::count(x.total))).unwrap_or_default()/>
                            <Stat label="In" value=i unit=iu detail=x.as_ref().map(|x| format::bytes(x.bytes_in)).unwrap_or_default()/>
                            <Stat label="Out" value=o unit=ou detail=x.as_ref().map(|x| format::bytes(x.bytes_out)).unwrap_or_default()/>
                            <Stat label="Refused by filters" value=x.as_ref().map(|x| format::count(x.refused)).unwrap_or_else(|| "0".into()) detail="allow_from / deny_from"/>
                        </Stats>
                    }
                    .into_any();
                }
                let mode = if clients_of(&svc).len() > 1 {
                    format!(
                        "{} · new connections go to healthy clients",
                        svc.get("mode")
                            .map(plain)
                            .unwrap_or_else(|| "failover".into())
                    )
                } else {
                    String::new()
                };
                view! {
                    <div class="mb-4">
                        <Card title="Health per client" sub=mode flush=true testid="health">
                            <Table head=vec!["Client", "Connected from", "Health", "Set aside"]>{rows}</Table>
                        </Card>
                    </div>
                }
                .into_any()
            }
        }
    }
}

fn served(name: &str) -> impl IntoView {
    let s = store();
    let name = name.to_string();
    move || {
        let doc = s.edit.doc();
        let st = s.status.get();
        let rows = doc
            .get("services")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter(|(_, v)| clients_of(v).contains(&name))
                    .map(|(n, v)| (n.clone(), v.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if rows.is_empty() {
            return view! { <EmptyState title="Serves no services" message="Pick this client in a service to publish one of its ports."/> }.into_any();
        }
        let rates = s.rates.get();
        view! {
            <Card flush=true testid="served">
                <Table head=vec!["Service", "Public", "Target", "Open", "In"]>
                    {rows.into_iter().map(|(n, v)| {
                        let x = st.as_ref().and_then(|x| x.services.iter().find(|c| c.name == n)).cloned();
                        let label = n.clone();
                        view! {
                            <tr>
                                <th scope="row"><Link to=Route::Detail("services".into(), n.clone()) class="font-semibold text-text no-underline hover:underline">{label}</Link></th>
                                <td class="font-mono">{v.get("bind").map(plain).unwrap_or_else(|| "private".into())}</td>
                                <td class="font-mono text-text-muted">{v.get("target").map(plain).unwrap_or_default()}</td>
                                <td class="text-right tabular-nums">{x.map(|x| x.open).unwrap_or(0)}</td>
                                <td class="text-right tabular-nums">{human_bytes(rates.service(&n).bytes_in)}</td>
                            </tr>
                        }
                    }).collect_view()}
                </Table>
            </Card>
        }
        .into_any()
    }
}

fn related(section: &str, name: &str) -> impl IntoView {
    let s = store();
    let (section, name) = (section.to_string(), name.to_string());
    move || {
        s.edit.draft.track();
        let c = crate::form::ctx_now()?;
        let refs = referrers(&c.schema, &c.doc, &entity(&section), &name);
        let title = match section.as_str() {
            "clients" => "Used by",
            "visitors" => "Services it may use",
            _ => "Referred to by",
        };
        let items = refs
            .into_iter()
            .filter_map(|(owner, field)| {
                let (info, n) = entity_of(&c.schema, &owner)?;
                let sec = info.path.first()?.clone();
                let label = format!("{} {n}", info.label.to_lowercase());
                Some(view! {
                    <li class="flex items-center gap-2">
                        <Link to=Route::Detail(sec, n.clone()) class="font-semibold text-text no-underline hover:underline">{label}</Link>
                        <span class="font-mono text-xs text-text-faint">{field}</span>
                    </li>
                })
            })
            .collect::<Vec<_>>();
        (!items.is_empty()).then(|| {
            view! {
                <div class="mt-6">
                    <Card title=title testid="detail-related">
                        <ul class="space-y-1.5">{items}</ul>
                    </Card>
                </div>
            }
        })
    }
}
