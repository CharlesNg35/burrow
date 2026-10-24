use burrow_admin_logic::web::SiteKind;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{Entry, Protocol, ServiceEntry, VisitorEntry};
use burrow_ui_kit::Tone;

use crate::form::plain;
use crate::format;
use crate::route::Route;
use crate::screens::Row;
use crate::screens::paths::{clients_of, online};

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
            let since = format::span(c.as_ref().and_then(|c| c.connected_secs).unwrap_or(0));
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
        meta: if me == Some(name.as_str()) && open == 0 {
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
    let mut tags = vec![(
        protocol.to_uppercase(),
        if udp { Tone::Violet } else { Tone::Accent },
    )];
    match burrow_admin_logic::web::site_kind(v, https_on) {
        Some(SiteKind::Https) => tags.push(("HTTPS".into(), Tone::Success)),
        Some(SiteKind::Plain) => tags.push(("HTTP · unencrypted".into(), Tone::Warning)),
        Some(SiteKind::Passthrough) => tags.push(("passthrough".into(), Tone::Neutral)),
        None => {}
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
        filters: vec![
            if public { "public" } else { "private" },
            if udp { Protocol::Udp } else { Protocol::Tcp }.wire(),
        ],
        actions: Vec::new(),
        ..Row::default()
    }
}
