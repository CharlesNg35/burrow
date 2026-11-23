use std::collections::HashMap;

use burrow_admin_logic::health::Level;
use burrow_admin_logic::presence::Presence;
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::{ClientStatus, Status};
use burrow_config::{Entry, ServiceEntry, entity_of_section};
use burrow_ui_kit::Tone;
use leptos::prelude::*;

use crate::format;
use crate::screens::Row;
use crate::store::store;

use super::HealthMark;
use super::index::Index;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Class {
    Service {
        dot: Tone,
        health: Option<HealthMark>,
        problem: bool,
    },
    Client {
        presence: Presence,
        host: String,
    },
    Visitor {
        in_use: bool,
    },
}

pub(super) type Classes = HashMap<String, Class>;

pub(super) const REMOVED: &str = "removed in your changes";

#[derive(Clone, Copy)]
pub(crate) struct LiveText(pub Callback<String, (String, String)>);

fn host_of(c: Option<&ClientStatus>) -> String {
    c.and_then(|c| c.from.as_deref())
        .map(burrow_admin_logic::presence::from_ip)
        .unwrap_or_default()
}

fn service_class(name: &str, clients: &[String], ix: &Index) -> Class {
    let count = ix.status.map(|_| {
        crate::screens::paths::health_of_clients(ix.service(name), clients.to_vec(), |c| {
            ix.connected(c)
        })
    });
    let problem = count.is_some_and(|c| c.level() != Level::Up);
    let up = clients.iter().filter(|c| ix.presence(c).up()).count();
    Class::Service {
        dot: if !problem {
            Tone::Success
        } else if up == 0 {
            Tone::Danger
        } else {
            Tone::Warning
        },
        health: count
            .map(super::health_mark)
            .filter(|m| m.1 != Tone::Success),
        problem,
    }
}

pub(super) fn classes(
    section: &str,
    names: &[(String, Vec<String>)],
    ix: &Index,
    me: Option<&str>,
) -> Classes {
    let kind = entity_of_section(section);
    names
        .iter()
        .map(|(name, clients)| {
            let class = match kind {
                Some(Entity::Client) => Class::Client {
                    presence: ix.presence(name),
                    host: host_of(ix.client(name)),
                },
                Some(Entity::Visitor) => Class::Visitor {
                    in_use: ix.visitor(name).is_some_and(|x| x.open > 0) || me == Some(name),
                },
                _ => service_class(name, clients, ix),
            };
            (name.clone(), class)
        })
        .collect()
}

pub(super) fn apply(mut row: Row, class: Option<&Class>) -> Row {
    match class {
        Some(Class::Service {
            dot,
            health,
            problem,
        }) => {
            row.dot = Some(*dot);
            row.health = health.clone();
            row.group = if *problem { "Problems" } else { "Up" }.into();
            row.problems += usize::from(matches!(dot, Tone::Danger | Tone::Warning));
        }
        Some(Class::Client { presence, host }) => {
            let on = presence.up();
            row.dot = format::tone_of(*presence).or(Some(Tone::Neutral));
            row.group = if on {
                "Online".into()
            } else {
                burrow_admin_logic::wording::capital(burrow_admin_logic::wording::OFFLINE)
            };
            row.filters = vec![if on {
                crate::screens::ONLINE_FILTER
            } else {
                crate::screens::OFFLINE_FILTER
            }];
            if !host.is_empty() {
                row.terms.push('\n');
                row.terms.push_str(host);
            }
        }
        Some(Class::Visitor { in_use }) => {
            row.dot = Some(if *in_use {
                Tone::Success
            } else {
                Tone::Neutral
            });
            row.group = if *in_use { "In use now" } else { "Not in use" }.into();
            if *in_use {
                row.filters.push("active");
            }
        }
        None => {}
    }
    row
}

fn client_text(c: Option<&ClientStatus>, on: bool, pending: bool) -> (String, String) {
    let meta = match (c, on) {
        (Some(c), true) => format!("{} open", c.open),
        (Some(c), false) => c
            .left_secs_ago
            .map(|a| format!("left {}", format::coarse_ago(a)))
            .unwrap_or_else(|| "not seen".into()),
        (None, _) => String::new(),
    };
    let line2 = if pending {
        burrow_admin_logic::wording::NOT_APPLIED.into()
    } else if on {
        let host = host_of(c);
        let since = format::connected_for(c.and_then(|c| c.connected_secs).unwrap_or(0));
        let mut line = if host.is_empty() {
            since
        } else {
            format!("{host} · {since}")
        };
        if let Some(v) = c
            .map(burrow_admin_logic::presence::info)
            .filter(|i| i.system.is_some())
        {
            line.push_str(&format!(" · {}", v.version));
        }
        line
    } else {
        String::new()
    };
    (meta, line2)
}

pub(super) fn service_line(v: &Value) -> String {
    format!(
        "{} → {}",
        burrow_admin_logic::web::reached(v),
        ServiceEntry::TARGET
            .get(v)
            .filter(|t| !t.is_null())
            .map_or_else(
                || burrow_admin_logic::wording::NOT_SET.to_string(),
                burrow_admin_logic::web::target_value_shown
            )
    )
}

fn text(section: &str, name: &str, status: Option<&Status>) -> (String, String) {
    let s = store();
    let (v, pending) = s.edit.draft.with(|d| {
        (
            d.draft().get(section).and_then(|m| m.get(name)).cloned(),
            d.base_name(section, name).is_none(),
        )
    });
    if v.is_none() {
        return (String::new(), REMOVED.into());
    }
    match entity_of_section(section) {
        Some(Entity::Client) => {
            let c = status.and_then(|st| burrow_admin_logic::presence::client(st, name));
            client_text(c, format::presence_of(status, name).up(), pending)
        }
        Some(Entity::Visitor) => {
            let me = s
                .welcome
                .with(|w| w.as_ref().map(|w| w.you.visitor.clone()));
            let st = status.and_then(|st| st.visitors.iter().find(|c| c.name == name));
            let open = st.map(|x| x.open).unwrap_or(0);
            let meta = if me.as_deref() == Some(name) && open == 0 {
                "you".into()
            } else if status.is_some() {
                format::visitor_use_short(open, st.and_then(|x| x.last_seen_secs))
            } else {
                String::new()
            };
            (meta, String::new())
        }
        _ => {
            let st = status.and_then(|st| st.services.iter().find(|x| x.name == name));
            (
                st.map(|x| format!("{} open", x.open)).unwrap_or_default(),
                v.as_ref().map(service_line).unwrap_or_default(),
            )
        }
    }
}

pub(crate) fn live_text(section: &str, name: &str) -> (String, String) {
    store().status.with(|st| text(section, name, st.as_deref()))
}

pub(crate) fn provide_live_text(section: String) {
    provide_context(LiveText(Callback::new(move |k: String| {
        live_text(&section, &k)
    })));
}
