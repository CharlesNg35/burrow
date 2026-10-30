use burrow_admin_logic::web::SiteKind;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{Entry, Protocol, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{Tone, tag_class};
use leptos::prelude::*;

use crate::form::plain;
use crate::format;
use crate::route::{Link, Route};
use crate::screens::Row;
use crate::screens::paths::{clients_of, online};

pub(crate) fn protocol_tag(svc: &Value) -> (String, Tone) {
    let p = ServiceEntry::PROTOCOL
        .get(svc)
        .map(plain)
        .unwrap_or_else(|| Protocol::default().wire().into());
    let tone = if Protocol::from_wire(&p) == Some(Protocol::Udp) {
        Tone::Violet
    } else {
        Tone::Accent
    };
    (p.to_uppercase(), tone)
}

pub(crate) fn protocol_view(svc: &Value) -> impl IntoView + use<> {
    let (text, tone) = protocol_tag(svc);
    view! { <span class=tag_class(tone)>{text}</span> }
}

pub(crate) fn service_tr(
    name: &str,
    svc: &Value,
    status: Option<&Status>,
    rate: Option<String>,
) -> impl IntoView + use<> {
    let x = status.and_then(|st| st.services.iter().find(|y| y.name == name));
    let cs = clients_of(svc);
    let up = status.map(|_| cs.iter().filter(|c| online(status, c)).count());
    let health = super::health_tag(up, cs.len(), x.map(|x| x.health.as_slice()).unwrap_or(&[]));
    let open = x.map(|x| format::count(x.open)).unwrap_or_default();
    let target = ServiceEntry::TARGET.get(svc).map(plain).unwrap_or_default();
    let name = name.to_string();
    let to = Route::Detail(ServiceEntry::section().into(), name.clone());
    let label = name.clone();
    let exposure = match burrow_admin_logic::web::exposure(svc) {
        Some(b) => view! { <span class="font-mono text-[13px]">{b}</span> }.into_any(),
        None => view! { <span class="font-mono text-[13px] text-text-faint">"private"</span> }
            .into_any(),
    };
    view! {
        <tr data-row=name>
            <th scope="row"><Link to=to class="font-semibold text-text no-underline hover:underline">{label}</Link></th>
            <td>{protocol_view(svc)}</td>
            <td>{exposure}</td>
            <td class="max-w-[260px] truncate font-mono text-[13px] text-text-muted" title=target.clone() data-target=target.clone()>{format!("→ {target}")}</td>
            <td class="text-right tabular-nums" data-cell="open">{open}</td>
            {rate.map(|r| view! { <td class="text-right tabular-nums" data-cell="rate">{r}</td> })}
            <td>{health}</td>
        </tr>
    }
}

pub(super) fn client_row(
    name: &String,
    draft: &Value,
    status: Option<&Status>,
    pending: Option<&'static str>,
    to: Route,
) -> Row {
    let c = status
        .as_ref()
        .and_then(|st| st.clients.iter().find(|c| &c.name == name))
        .cloned();
    let on = c.as_ref().is_some_and(|c| c.connected);
    let served: Vec<String> = ServiceEntry::entries(draft)
        .map(|m| {
            m.iter()
                .filter(|(_, x)| clients_of(x).contains(name))
                .map(|(n, _)| n.clone())
                .collect()
        })
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
            let since = format::span(c.as_ref().and_then(|c| c.connected_secs).unwrap_or(0));
            if host.is_empty() {
                format!("for {since}")
            } else {
                format!("{host} · for {since}")
            }
        } else {
            String::new()
        },
        terms: served.join(" "),
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

pub(super) fn visitor_row(
    name: &String,
    v: &Value,
    draft: &Value,
    status: Option<&Status>,
    me: Option<&str>,
    pending: Option<&'static str>,
    to: Route,
) -> Row {
    let st = status
        .as_ref()
        .and_then(|st| st.visitors.iter().find(|c| &c.name == name))
        .cloned();
    let open = st.as_ref().map(|x| x.open).unwrap_or(0);
    let seen = st.as_ref().and_then(|x| x.last_seen_secs);
    let in_use = open > 0 || me == Some(name.as_str());
    let admin = VisitorEntry::ADMIN.get(v).map(plain);
    let mut filters = vec![];
    if admin.is_some() {
        filters.push("admins");
    }
    if in_use {
        filters.push("active");
    }
    let may: Vec<String> = ServiceEntry::entries(draft)
        .map(|m| {
            m.iter()
                .filter(|(_, x)| {
                    ServiceEntry::VISITORS
                        .get(x)
                        .and_then(Value::as_array)
                        .is_some_and(|a| a.iter().any(|y| y.as_str() == Some(name.as_str())))
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
    Row {
        key: name.clone(),
        to,
        icon: if admin.is_some() { "shield" } else { "person" },
        dot: Some(if in_use { Tone::Success } else { Tone::Neutral }),
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
        meta: if me == Some(name.as_str()) && open == 0 {
            "you".into()
        } else if status.is_some() {
            format::visitor_use_short(open, seen)
        } else {
            String::new()
        },
        terms: may
            .iter()
            .chain(&reach)
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
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

pub(super) fn service_row(
    name: &String,
    (v, https_on): (&Value, bool),
    status: Option<&Status>,
    pending: Option<&'static str>,
    to: Route,
) -> Row {
    let st = status
        .as_ref()
        .and_then(|st| st.services.iter().find(|c| &c.name == name))
        .cloned();
    let public = burrow_admin_logic::web::is_public(v);
    let ways = burrow_admin_logic::web::ways(v);
    let protocol = ServiceEntry::PROTOCOL
        .get(v)
        .map(plain)
        .unwrap_or_else(|| Protocol::default().wire().into());
    let udp = Protocol::from_wire(&protocol) == Some(Protocol::Udp);
    let cs = clients_of(v);
    let up = cs.iter().filter(|c| online(status, c)).count();
    let health_up = st
        .as_ref()
        .map(|x| (x.health.iter().filter(|h| h.up).count(), x.health.len()));
    let problem =
        status.is_some() && (up < cs.len() || health_up.is_some_and(|(u, n)| n > 0 && u < n));
    let mut tags = vec![protocol_tag(v)];
    let site = burrow_admin_logic::web::site_kind(v, https_on);
    match site {
        Some(SiteKind::Https) => tags.push(("HTTPS".into(), Tone::Success)),
        Some(SiteKind::Plain) => tags.push(("HTTP · unencrypted".into(), Tone::Warning)),
        Some(SiteKind::Passthrough) => tags.push(("passthrough".into(), Tone::Neutral)),
        None => {}
    }
    if ways.public {
        tags.push(("public".into(), Tone::Neutral));
    }
    if ways.visitors {
        tags.push((ServiceEntry::VISITORS.key().into(), Tone::Neutral));
    }
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
            burrow_admin_logic::web::exposure(v).unwrap_or_else(|| "private".into()),
            plain(ServiceEntry::TARGET.get(v).unwrap_or(&Value::Null))
        ),
        tags,
        group: if problem {
            "Problems".into()
        } else {
            "Up".into()
        },
        pending,
        filters: [
            ways.public.then_some(crate::route::PUBLIC_FILTER),
            Some(if udp { Protocol::Udp } else { Protocol::Tcp }.wire()),
            site.is_some().then_some(crate::route::WEBSITES_FILTER),
            ways.visitors.then_some(ServiceEntry::VISITORS.key()),
        ]
        .into_iter()
        .flatten()
        .collect(),
        actions: Vec::new(),
        ..Row::default()
    }
}
