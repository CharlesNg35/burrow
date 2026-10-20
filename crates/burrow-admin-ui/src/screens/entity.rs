use burrow_admin_logic::changeset::{get, ops_between};
use burrow_admin_logic::rates::human_bits;
use burrow_admin_proto::Path;
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

pub fn rows_for(section: &str) -> Vec<Row> {
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
                let uptime = status.as_ref().map(|s| s.uptime_secs).unwrap_or(0);
                let serves = if served.is_empty() {
                    "no services".to_string()
                } else {
                    served.join(", ")
                };
                let host = c
                    .as_ref()
                    .and_then(|c| c.from.clone())
                    .map(|f| f.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or(f))
                    .unwrap_or_default();
                let left = c.as_ref().and_then(|c| c.left_secs_ago);
                Row {
                    key: name.clone(),
                    to,
                    icon: "monitor",
                    dot: Some(if on { Tone::Success } else { Tone::Neutral }),
                    title: name.clone(),
                    meta: match (&c, on) {
                        (Some(c), true) => format!("{} open", c.open),
                        (Some(_), false) => {
                            left.map(format::ago).unwrap_or_else(|| "not seen".into())
                        }
                        (None, _) => String::new(),
                    },
                    line2: if on {
                        format!(
                            "{host} · for {} · {serves}",
                            format::span(c.as_ref().and_then(|c| c.connected_secs).unwrap_or(0))
                        )
                    } else if pending.is_some() {
                        "Draft · not applied yet".into()
                    } else {
                        format!("offline · {serves}")
                    },
                    line3: if on {
                        format!(
                            "{} idle channels",
                            c.as_ref().map(|c| c.idle_channels).unwrap_or(0)
                        )
                    } else if status.is_some() && pending.is_none() {
                        format::left(left, uptime)
                    } else {
                        String::new()
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
            "visitors" => {
                let st = status
                    .as_ref()
                    .and_then(|st| st.visitors.iter().find(|c| &c.name == name))
                    .cloned();
                let open = st.as_ref().map(|x| x.open).unwrap_or(0);
                let admin = v.get("admin").map(plain);
                let mut filters = vec![];
                if admin.is_some() {
                    filters.push("admins");
                }
                if open > 0 {
                    filters.push("active");
                }
                let may: Vec<String> = draft
                    .get("services")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .filter(|(_, x)| {
                                x.get("visitors")
                                    .and_then(Value::as_array)
                                    .is_some_and(|a| {
                                        a.iter().any(|y| y.as_str() == Some(name.as_str()))
                                    })
                            })
                            .map(|(n, _)| n.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let reach: Vec<String> = v
                    .get("reach")
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
                    icon: "person",
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
                    meta: format!("{open} open"),
                    line2,
                    tags: if open > 0 {
                        vec![("active".to_string(), Tone::Success)]
                    } else {
                        Vec::new()
                    },
                    aside: format!(
                        "{} total",
                        format::count(st.as_ref().map(|x| x.total).unwrap_or(0))
                    ),
                    group: if open > 0 {
                        "Active".into()
                    } else {
                        "Idle".into()
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
                    tags.push(("✕ Down".into(), Tone::Danger));
                } else if let Some((u, n)) = health_up.filter(|(u, n)| *n > 0 && u < n) {
                    tags.push((format!("⚠ {u} of {n} up"), Tone::Warning));
                }
                let aside = st
                    .as_ref()
                    .map(|x| format!("{} total", format::count(x.total)))
                    .unwrap_or_default();
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
                    aside,
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
                    ..Row::default()
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
            vec!["Active", "Idle"],
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
                        icon: if what == "client" {
                            "monitor"
                        } else if what == "visitor" {
                            "person"
                        } else {
                            "services"
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
                    <DetailPane>
                        <EmptyState icon="services" title=pick message="Or add a new one."/>
                    </DetailPane>
                }
                .into_any(),
            }
        }}
    }
    .into_any();
    master_detail(list, detail)
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
        .with_untracked(|d| get(d.base(), &path).is_none() && get(d.draft(), &path).is_some())
        && crate::secrets::revealed_for(&section, &name).is_none();
    let editing = RwSignal::new(fresh && s.can_write_now());
    Effect::new(move |_| {
        let on = editing.get();
        let _ = s.editing.try_set(on);
    });
    on_cleanup(move || {
        let _ = s.editing.try_set(false);
    });
    let snapshot: RwSignal<Option<Value>> = RwSignal::new(None);
    let read_only = !s.can_write_now();
    let tab: RwSignal<&'static str> = RwSignal::new(match route::query("tab").as_deref() {
        Some("settings") => "settings",
        Some("access") => "access",
        Some("secrets") => "secrets",
        Some("services") => "services",
        Some("connect") => "connect",
        _ if fresh => "settings",
        _ if crate::secrets::revealed_for(&section, &name).is_some() => "connect",
        _ => "overview",
    });
    {
        let (sec, nm) = (section.clone(), name.clone());
        Effect::new(move |was: Option<bool>| {
            let now = crate::secrets::revealed_for(&sec, &nm).is_some();
            if now && was == Some(false) {
                tab.set("connect");
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
    let mode = EditMode(Signal::derive(move || {
        editing.get() || (read_only && matches!(tab.get(), "settings" | "access"))
    }));
    let renaming = RwSignal::new(false);
    let menu = RwSignal::new(false);
    crate::app::popover(menu, "entity-more", "entity-more-menu");
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
            "visitors" => {
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
                let rate = s.rates.with(|r| r.service(&nm).bytes_in);
                paths::service(
                    &nm,
                    &svc,
                    status.as_deref(),
                    rate,
                    &problems_for(&vec![sec.clone(), nm.clone()]),
                    editing.get(),
                    &crate::domain::reserved_clients(),
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
                    let up = st.as_ref().map(|s| s.uptime_secs).unwrap_or(0);
                    format!(
                        "{edit}Client · {}",
                        match c {
                            Some(c) if c.connected => format!(
                                "connected from {} for {}",
                                c.from.unwrap_or_default(),
                                format::span(c.connected_secs.unwrap_or(0))
                            ),
                            Some(c) => format!("offline · {}", format::left(c.left_secs_ago, up)),
                            None => "status not known yet".into(),
                        }
                    )
                }
                "visitors" => {
                    let v = st
                        .as_ref()
                        .and_then(|x| x.visitors.iter().find(|c| c.name == nm))
                        .cloned();
                    let open = v.as_ref().map(|v| v.open).unwrap_or(0);
                    let fp = doc
                        .get("visitors")
                        .and_then(|m| m.get(&nm))
                        .and_then(|x| x.get("secret"))
                        .and_then(|x| match x {
                            Value::Array(a) => a.first().cloned(),
                            other => Some(other.clone()),
                        })
                        .and_then(|x| as_secret(&x).and_then(|s| s.fp.clone()));
                    format!(
                        "{edit}Visitor · {} · {open} open · {} total{}",
                        if open > 0 { "active" } else { "idle" },
                        format::count(v.as_ref().map(|v| v.total).unwrap_or(0)),
                        fp.map(|f| format!(" · secret {f}")).unwrap_or_default()
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
        move || match sec.as_str() {
            "services" => {
                let p = e
                    .doc()
                    .get("services")
                    .and_then(|m| m.get(&nm))
                    .and_then(|v| v.get("protocol"))
                    .map(plain)
                    .unwrap_or_else(|| "tcp".into());
                let tone = if p == "udp" { Tone::Udp } else { Tone::Accent };
                view! { <span class=tag_class(tone)>{p.to_uppercase()}</span> }.into_any()
            }
            "visitors" => {
                let admin = e
                    .doc()
                    .get("visitors")
                    .and_then(|m| m.get(&nm))
                    .and_then(|v| v.get("admin"))
                    .map(plain);
                let me = s
                    .welcome
                    .with(|w| w.as_ref().is_some_and(|w| w.you.visitor == nm));
                view! {
                        {admin.map(|a| {
                            let tone = if a == "write" { Tone::Danger } else { Tone::Accent };
                            view! { <span class=tag_class(tone)><Icon name=if a == "write" { "shield" } else { "eye" } small=true/>{format!("admin · {a}")}</span> }
                        })}
                        {me.then(|| view! { <span class=tag_class(Tone::Neutral) data-testid="you-tag">"you"</span> })}
                    }
                    .into_any()
            }
            _ => ().into_any(),
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
    let tab_counts = {
        let (sec, nm, keys) = (
            section.clone(),
            name.clone(),
            tabs.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        );
        Signal::derive(move || {
            let doc = e.doc();
            let me = doc
                .get(&sec)
                .and_then(|m| m.get(&nm))
                .cloned()
                .unwrap_or(Value::Null);
            let list = |v: Option<&Value>| match v {
                Some(Value::Array(a)) => a.len(),
                Some(Value::Object(m)) => m.len(),
                Some(Value::Null) | None => 0,
                Some(_) => 1,
            };
            let served = doc
                .get("services")
                .and_then(Value::as_object)
                .map(|m| m.values().filter(|v| clients_of(v).contains(&nm)).count())
                .unwrap_or(0);
            let may = doc
                .get("services")
                .and_then(Value::as_object)
                .map(|m| {
                    m.values()
                        .filter(|v| {
                            v.get("visitors")
                                .and_then(Value::as_array)
                                .is_some_and(|a| a.iter().any(|y| y.as_str() == Some(nm.as_str())))
                        })
                        .count()
                })
                .unwrap_or(0);
            keys.iter()
                .map(|k| {
                    let n = match (sec.as_str(), *k) {
                        (_, "secrets") => secrets(&me),
                        ("clients", "services") => served,
                        ("visitors", "access") => may + list(me.get("reach")),
                        ("services", "access") => {
                            list(me.get("visitors"))
                                + list(me.get("allow_from"))
                                + list(me.get("deny_from"))
                        }
                        _ => 0,
                    };
                    if n > 0 { n.to_string() } else { String::new() }
                })
                .collect()
        })
    };
    let can = s.can_write_now();
    let head_actions = {
        let start_edit = start_edit.clone();
        let (path, sec, nm) = (path.clone(), section.clone(), name.clone());
        move || {
            if editing.get() {
                return view! { <span class=tag_class(Tone::Accent) data-testid="editing-tag"><Icon name="edit" small=true/>"Editing"</span> }.into_any();
            }
            if crate::secrets::revealed_for(&sec, &nm).is_some() {
                return view! { <span class=tag_class(Tone::Success) data-testid="applied-tag"><Icon name="check" small=true/>"Applied"</span> }.into_any();
            }
            let start_edit = start_edit.clone();
            let (path, sec, nm) = (path.clone(), sec.clone(), nm.clone());
            let back = Route::List(sec.clone());
            let connect = (sec == "clients").then(|| view! {
                <Button label="Connect" icon="terminal" variant="secondary" testid="connect-head" on_click=move |_| tab.set("connect")/>
            });
            view! {
                {connect}
                <Button
                    label="Edit"
                    icon="edit"
                    testid="edit"
                    disabled=!can
                    reason=if can { String::new() } else { format!("Editing needs write admin. {} has read.", s.welcome.with_untracked(|w| w.as_ref().map(|w| w.you.visitor.clone()).unwrap_or_default())) }
                    on_click=move |_| start_edit()
                />
                <div class="relative">
                    <button type="button" class=button_class("secondary", false) aria-label="More actions" aria-haspopup="true" aria-expanded=move || menu.get().to_string() aria-controls="entity-more-menu" id="entity-more" data-testid="more" on:click=move |_| menu.update(|m| *m = !*m)>
                        <Icon name="more"/>
                    </button>
                    <Show when=move || menu.get()>
                        {
                            let (path, nm, back, sec) = (path.clone(), nm.clone(), back.clone(), sec.clone());
                            view! {
                                <div class="absolute top-10 right-0 z-30 w-48 rounded-[10px] border border-border-strong bg-surface p-1 shadow-overlay outline-none dark:bg-surface-2" id="entity-more-menu" tabindex="-1" data-testid="more-menu" on:click=move |_| menu.set(false)>
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
    let edit_cb = {
        let start_edit = start_edit.clone();
        Callback::new(move |_: ()| start_edit())
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
                    ("visitors", "overview") => visitor_panels(&nm2).into_any(),
                    ("clients", "overview") => {
                        let secrets = crate::secrets::secret_list(
                            vec![sec2.clone(), nm2.clone(), "secret".into()],
                            "overview-secrets".into(),
                            4,
                        );
                        let card = settings_card(
                            &node,
                            path2.clone(),
                            spath.clone(),
                            &["secret"],
                            edit_cb,
                        );
                        view! { {secrets} {served(&nm2)} {card} }.into_any()
                    }
                    (_, "overview") => {
                        let card = settings_card(&node, path2.clone(), spath.clone(), &[], edit_cb);
                        view! { {overview(&sec2, &nm2, false)} {card} }.into_any()
                    }
                    ("clients", "secrets") | ("visitors", "secrets") => {
                        let p = vec![sec2.clone(), nm2.clone(), "secret".into()];
                        crate::secrets::secret_list(p, "secrets".into(), 4)
                    }
                    ("clients", "services") => served(&nm2).into_any(),
                    (_, "connect") => {
                        if crate::secrets::revealed_for(&sec2, &nm2).is_some() {
                            view! { <crate::secrets::ShownOnce section=sec2.clone() name=nm2.clone()/> }.into_any()
                        } else {
                            view! { <crate::secrets::ConnectPanel section=sec2.clone() name=nm2.clone()/> }.into_any()
                        }
                    }
                    (_, "access") => {
                        let keep: &[&str] = if sec2 == "visitors" {
                            &["reach", "max_connections", "admin", "admin_from"]
                        } else {
                            &["visitors", "allow_from", "deny_from"]
                        };
                        let skip: Vec<&str> = burrow_admin_logic::tree::fields(&node)
                            .iter()
                            .map(|f| f.key.as_str())
                            .filter(|k| !keep.contains(k))
                            .collect();
                        crate::form::object_view_skipping(
                            &node,
                            path2.clone(),
                            spath.clone(),
                            path2.clone(),
                            &skip,
                        )
                    }
                    _ => object_view(&node, path2.clone(), spath.clone(), path2.clone()),
                }
            };
            let nm_p = nm.clone();
            Some(view! {
                <PathDiagram data=path_data testid="path"/>
                <div class:max-sm:hidden=move || editing.get()>{overview(&sec, &nm, true)}</div>
                <Tabs tabs=tabs.clone() active=tab counts=tab_counts/>
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
            }
            .into_any())
        }
    };
    let save_bar = {
        let discard = discard.clone();
        move || {
            if read_only && matches!(tab.get(), "settings" | "access") {
                return Some(view! {
                    <div class="sticky bottom-4 z-20 mt-6 flex items-center gap-3 rounded-card border border-border bg-surface px-4 py-3 shadow-overlay" data-testid="save-bar" data-sticky-bar="">
                        <span class="flex items-center gap-1.5 text-sm text-text-muted"><Icon name="lock" small=true/>"Read-only: changes need write admin."</span>
                        <span class="ml-auto"></span>
                        <button type="button" class=button_class("primary", false) disabled=true data-testid="save">
                            <Icon name="lock" small=true/>
                            "Save to changes"
                        </button>
                    </div>
                }
                .into_any());
            }
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
                .into_any()
            })
        }
    };
    let title_dot = {
        let (sec, nm) = (section.clone(), name.clone());
        move || {
            (sec == "clients").then(|| {
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
            <header class="mb-[22px] flex items-start gap-3.5" class:max-sm:hidden=move || editing.get()>
                <div class="min-w-0 flex-1">
                    <h1 class="flex items-center gap-2.5 text-[25px] font-bold tracking-[-0.015em]">{title_dot}{name.clone()}{protocol_tag}</h1>
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

fn pool_range(doc: &Value) -> String {
    let p = doc.get("pool");
    let min = p
        .and_then(|p| p.get("min_idle"))
        .and_then(Value::as_u64)
        .unwrap_or(4);
    let max = p
        .and_then(|p| p.get("max_idle"))
        .and_then(Value::as_u64)
        .unwrap_or(64);
    format!("pool {min}–{max}")
}

fn since_date(secs: u64) -> String {
    let at = js_sys::Date::now() - secs as f64 * 1000.0;
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at));
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "since {} {:02}",
        months[d.get_month() as usize % 12],
        d.get_date()
    )
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
                if !stats {
                    return ().into_any();
                }
                let c = st
                    .as_ref()
                    .and_then(|x| x.clients.iter().find(|c| c.name == name))
                    .cloned();
                let n = doc
                    .get("clients")
                    .and_then(|m| m.get(&name))
                    .map(secrets)
                    .unwrap_or(0);
                let max = doc
                    .get("clients")
                    .and_then(|m| m.get(&name))
                    .and_then(|v| v.get("max_connections"))
                    .and_then(Value::as_u64)
                    .or_else(|| {
                        doc.get("limits")
                            .and_then(|l| l.get("max_connections_per_client"))
                            .and_then(Value::as_u64)
                    })
                    .unwrap_or(8192);
                let connected = c.as_ref().and_then(|c| c.connected_secs);
                view! {
                    <Stats testid="live">
                        <Stat label="Open" value=c.as_ref().map(|c| format::count(c.open)).unwrap_or_else(|| "0".into()) detail=format!("max {}", format::count(max))/>
                        <Stat label="Idle channels" value=c.as_ref().map(|c| c.idle_channels.to_string()).unwrap_or_else(|| "0".into()) detail=pool_range(&doc)/>
                        <Stat
                            label="Connected"
                            value=connected.map(format::span).unwrap_or_else(|| "—".into())
                            detail=connected.map(since_date).unwrap_or_else(|| c.as_ref().map(|c| format::left(c.left_secs_ago, st.as_ref().map(|s| s.uptime_secs).unwrap_or(0))).unwrap_or_default())
                        />
                        <Stat label="Secrets" value=n.to_string() unit="/ 4" detail={ if n > 1 { "rotation in progress" } else { "" } }/>
                    </Stats>
                }
                .into_any()
            }
            "visitors" => ().into_any(),
            _ => {
                let x = st
                    .as_ref()
                    .and_then(|x| x.services.iter().find(|c| c.name == name))
                    .cloned();
                if stats {
                    let r = s.rates.get().service(&name);
                    let (i, iu) = human_bits(r.bytes_in);
                    let (o, ou) = human_bits(r.bytes_out);
                    let limit = doc
                        .get("services")
                        .and_then(|m| m.get(&name))
                        .and_then(|v| v.get("bandwidth"))
                        .map(|b| format!("limit {}", plain(b)));
                    return view! {
                        <Stats testid="live">
                            <Stat label="Open" value=x.as_ref().map(|x| format::count(x.open)).unwrap_or_else(|| "0".into()) detail=x.as_ref().map(|x| format!("{} total", format::count(x.total))).unwrap_or_default()/>
                            <Stat label="In" value=i unit=iu detail=limit.clone().unwrap_or_else(|| x.as_ref().map(|x| format::bytes(x.bytes_in)).unwrap_or_default())/>
                            <Stat label="Out" value=o unit=ou detail=limit.unwrap_or_else(|| x.as_ref().map(|x| format::bytes(x.bytes_out)).unwrap_or_default())/>
                            <Stat label="Refused by filters" value=x.as_ref().map(|x| format::count(x.refused)).unwrap_or_else(|| "0".into()) detail="allow_from / deny_from"/>
                        </Stats>
                    }
                    .into_any();
                }
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
                                <td class="font-mono text-[13px] text-text-muted">{cs.and_then(|y| y.from).unwrap_or_else(|| "offline".into())}</td>
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
                let mode = match (
                    clients_of(&svc).len() > 1,
                    svc.get("mode").map(plain).as_deref(),
                ) {
                    (true, Some("balance")) => {
                        "balance · new connections spread over healthy clients".to_string()
                    }
                    (true, _) => {
                        "failover · the first healthy client takes new connections".to_string()
                    }
                    _ => String::new(),
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

fn settings_card(
    node: &burrow_admin_proto::schema::Node,
    path: Path,
    spath: Path,
    skip: &[&str],
    on_edit: Callback<()>,
) -> AnyView {
    let facts = crate::form::facts(node, path, spath, skip);
    let can = store().can_write_now();
    view! {
        <section class="mb-4 rounded-card border border-border bg-surface" data-testid="settings-summary">
            <div class="flex min-h-12 items-center gap-3 border-b border-border px-4 py-3">
                <h2 class="text-[13.5px] font-bold">"Settings"</h2>
                {can.then(|| view! {
                    <button type="button" class="ml-auto cursor-pointer text-[13px] font-semibold text-accent-ink hover:underline" data-testid="settings-edit" on:click=move |_| on_edit.run(())>"Edit"</button>
                })}
            </div>
            {facts}
        </section>
    }
    .into_any()
}

fn visitor_panels(name: &str) -> impl IntoView {
    let s = store();
    let name = name.to_string();
    move || {
        let doc = s.edit.doc();
        let st = s.status.get();
        let v = doc
            .get("visitors")
            .and_then(|m| m.get(&name))
            .cloned()
            .unwrap_or(Value::Null);
        let level = v.get("admin").map(plain);
        let from: Vec<String> = v
            .get("admin_from")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(plain).collect())
            .unwrap_or_default();
        let may: Vec<(String, Option<bool>)> = doc
            .get("services")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter(|(_, x)| {
                        x.get("visitors")
                            .and_then(Value::as_array)
                            .is_some_and(|a| a.iter().any(|y| y.as_str() == Some(name.as_str())))
                    })
                    .map(|(n, x)| {
                        let up = st.as_ref().map(|st| {
                            clients_of(x)
                                .iter()
                                .any(|c| st.clients.iter().any(|y| &y.name == c && y.connected))
                        });
                        (n.clone(), up)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let reach: Vec<(String, Value)> = v
            .get("reach")
            .and_then(Value::as_object)
            .map(|m| m.iter().map(|(k, x)| (k.clone(), x.clone())).collect())
            .unwrap_or_default();
        let level_view = match level.as_deref() {
            Some("write") => view! { <span class=tag_class(Tone::Danger)>"write"</span><span class="ml-2">"may change the whole server config"</span> }.into_any(),
            Some("read") => view! { <span class=tag_class(Tone::Accent)>"read"</span><span class="ml-2">"may see everything, with secrets as fingerprints"</span> }.into_any(),
            _ => view! { <span class="text-text-muted">"none: no admin access"</span> }.into_any(),
        };
        let from_view = if from.is_empty() {
            view! { <span class="text-text-muted">"any address"</span> }.into_any()
        } else {
            from.into_iter()
                .map(|c| {
                    let size = crate::domain::cidr_size(&c);
                    view! { <span class="mr-1.5 inline-flex h-[26px] items-center gap-1.5 rounded-sm bg-surface-3 px-2 font-mono text-[12.5px] font-semibold ring-1 ring-border ring-inset">{c}<span class="font-sans text-[11px] font-normal text-text-muted">{size}</span></span> }
                })
                .collect_view()
                .into_any()
        };
        let chips = if may.is_empty() {
            view! { <p class="px-4 py-3 text-sm text-text-muted">"No private services name this visitor."</p> }.into_any()
        } else {
            view! {
                <div class="flex flex-wrap gap-2 px-4 py-3">
                    {may.into_iter().map(|(n, up)| {
                        let dot = match up {
                            Some(true) => "bg-success",
                            Some(false) => "bg-transparent ring-[1.5px] ring-text-faint ring-inset",
                            None => "bg-text-faint",
                        };
                        view! {
                            <Link to=Route::Detail("services".into(), n.clone()) class="inline-flex h-[26px] items-center gap-1.5 rounded-sm bg-surface-3 px-2 text-[12.5px] font-semibold text-text no-underline ring-1 ring-border ring-inset">
                                <span class=format!("size-2 rounded-full {dot}") aria-hidden="true"></span>
                                {n.clone()}
                            </Link>
                        }
                    }).collect_view()}
                </div>
            }
            .into_any()
        };
        let direct = (!reach.is_empty()).then(|| {
            let rows = reach
                .into_iter()
                .map(|(c, rules)| {
                    let on = st.as_ref().is_some_and(|st| st.clients.iter().any(|y| y.name == c && y.connected));
                    let rules_view = match &rules {
                        Value::Array(a) => a
                            .iter()
                            .map(|r| view! { <span class="mr-1.5 inline-flex h-[26px] items-center rounded-sm bg-surface-3 px-2 font-mono text-[12.5px] font-semibold ring-1 ring-border ring-inset">{plain(r)}</span> })
                            .collect_view()
                            .into_any(),
                        other => view! { <span class=tag_class(Tone::Warning)>{format!("{} target", plain(other))}</span> }.into_any(),
                    };
                    let label = c.clone();
                    view! {
                        <tr data-row=c.clone()>
                            <th scope="row"><StatusDot on=on text=label/></th>
                            <td>{rules_view}</td>
                        </tr>
                    }
                })
                .collect_view();
            view! {
                <div class="mb-4">
                    <Card title="Direct access" flush=true testid="direct-access" head=std::sync::Arc::new(|| view! { <span>"reach"</span> }.into_any()) as ChildrenFn>
                        <Table head=vec!["Client", "Rules"]>{rows}</Table>
                    </Card>
                </div>
            }
        });
        view! {
            <section class="mb-4 rounded-card border border-border bg-surface" data-testid="admin-access">
                <div class="border-b border-border px-4 py-3"><h2 class="text-[13.5px] font-bold">"Admin access"</h2></div>
                <div class="px-4 py-1">
                    <div class="grid grid-cols-[170px_minmax(0,1fr)] items-center gap-4 border-b border-rowline py-2.5 text-[13.5px]"><span class="text-text-muted">"Level"</span><span>{level_view}</span></div>
                    <div class="grid grid-cols-[170px_minmax(0,1fr)] items-center gap-4 py-2.5 text-[13.5px]"><span class="text-text-muted">"Admin from"</span><span>{from_view}</span></div>
                </div>
            </section>
            <section class="mb-4 rounded-card border border-border bg-surface" data-testid="may-use">
                <div class="border-b border-border px-4 py-3"><h2 class="text-[13.5px] font-bold">"Services it may use"</h2></div>
                {chips}
            </section>
            {direct}
        }
        .into_any()
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
            return view! { <div class="mb-4"><EmptyState title="Serves no services" message="Pick this client in a service to publish one of its ports."/></div> }.into_any();
        }
        view! {
            <div class="mb-4">
                <Card title="Services it serves" flush=true testid="served">
                    <Table head=vec!["Service", "_", "Public", "Target", ">Open", "_Health"]>
                        {rows.into_iter().map(|(n, v)| {
                            let x = st.as_ref().and_then(|x| x.services.iter().find(|c| c.name == n)).cloned();
                            let protocol = v.get("protocol").map(plain).unwrap_or_else(|| "tcp".into());
                            let up = st.as_ref().map(|st| clients_of(&v).iter().filter(|c| st.clients.iter().any(|y| &y.name == *c && y.connected)).count());
                            let failing = x.as_ref().map(|x| x.health.iter().filter(|h| !h.up).count()).unwrap_or(0);
                            let health = match up {
                                None => view! { <span class="text-text-faint">"—"</span> }.into_any(),
                                Some(0) => view! { <span class=tag_class(Tone::Danger)><Icon name="x" small=true/>"Down"</span> }.into_any(),
                                Some(_) if failing > 0 => view! { <span class=tag_class(Tone::Warning)><Icon name="alert" small=true/>{format!("{failing} failing")}</span> }.into_any(),
                                Some(_) => view! { <span class=tag_class(Tone::Success)><Icon name="check" small=true/>"Healthy"</span> }.into_any(),
                            };
                            let label = n.clone();
                            view! {
                                <tr data-row=n.clone()>
                                    <th scope="row"><Link to=Route::Detail("services".into(), n.clone()) class="font-semibold text-text no-underline hover:underline">{label}</Link></th>
                                    <td><span class=tag_class(if protocol == "udp" { Tone::Udp } else { Tone::Accent })>{protocol.to_uppercase()}</span></td>
                                    <td>{match v.get("bind").filter(|b| !b.is_null()) {
                                        Some(b) => view! { <span class="font-mono text-[13px]">{plain(b)}</span> }.into_any(),
                                        None => view! { <span class="font-mono text-[13px] text-text-faint">"private"</span> }.into_any(),
                                    }}</td>
                                    <td class="font-mono text-[13px] text-text-muted">{format!("→ {}", v.get("target").map(plain).unwrap_or_default())}</td>
                                    <td class="text-right tabular-nums">{x.map(|x| x.open).unwrap_or(0)}</td>
                                    <td>{health}</td>
                                </tr>
                            }
                        }).collect_view()}
                    </Table>
                </Card>
            </div>
        }
        .into_any()
    }
}
