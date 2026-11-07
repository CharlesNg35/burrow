//! The create screens for new entries and websites, with the path preview of a new entry.

mod https_choice;
mod new;
mod website;

use burrow_admin_proto::schema::Entity;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{Edge, EdgeState, Hop, HopState, PathData, Tone};
use leptos::prelude::*;

use crate::entities::singular;
use crate::store::store;

pub(crate) use new::NewEntity;
pub(crate) use website::NewWebsite;

fn services_for(section: &str) -> Vec<burrow_ui_kit::ComboOption> {
    let s = store();
    let status = s.status.get();
    burrow_admin_logic::choices::service_choices(&s.edit.doc(), status.as_deref())
        .into_iter()
        .filter(|c| section == ClientEntry::section() || c.private)
        .map(crate::addr::service_option)
        .collect()
}

pub fn new_path(section: &str, name: &str, host: &str, picked: usize) -> PathData {
    let shown = if name.is_empty() {
        format!(
            "new {}",
            burrow_admin_logic::entity::of_section(section).singular
        )
    } else {
        name.to_string()
    };
    let pending = |icon: &'static str, head: &str, value: &str| Hop {
        icon,
        head: head.into(),
        value: value.into(),
        dot: Some(Tone::Neutral),
        unit: String::new(),
        ..Hop::default()
    };
    let added = |above: &str| Edge {
        above: above.into(),
        below: String::new(),
        state: EdgeState::Added,
    };
    if section == ServiceEntry::section() {
        return PathData {
            hops: vec![
                pending("globe", "Who connects", "public or visitors"),
                Hop {
                    icon: "box",
                    head: "Server".into(),
                    value: shown,
                    mono: true,
                    dot: Some(Tone::Neutral),
                    unit: "on this server".into(),
                    state: HopState::Current,
                    badge: None,
                },
                pending(
                    burrow_admin_logic::entity::CLIENT_ICON,
                    "Client",
                    burrow_admin_logic::wording::NOT_SET,
                ),
                pending("fork", "Target", burrow_admin_logic::wording::NOT_SET),
            ],
            edges: vec![added(""), added(""), added("")],
            summary:
                "A new service: who connects, this server, its client and its target, set next"
                    .into(),
        };
    }
    let visitor = section == VisitorEntry::section();
    PathData {
        hops: vec![
            Hop {
                icon: burrow_admin_logic::entity::info(if visitor {
                    &Entity::Visitor
                } else {
                    &Entity::Client
                })
                .icon,
                head: singular(section).to_uppercase(),
                value: shown,
                mono: true,
                dot: Some(Tone::Neutral),
                unit: "not connected yet".into(),
                state: HopState::Current,
                badge: None,
            },
            Hop {
                icon: "box",
                head: "Server".into(),
                value: host.into(),
                mono: true,
                dot: Some(Tone::Success),
                unit: "this server".into(),
                ..Hop::default()
            },
            Hop {
                icon: "stack",
                head: "Services".into(),
                value: match picked {
                    0 => "none".into(),
                    1 => "1 service".into(),
                    n => format!("{n} services"),
                },
                dot: (picked == 0).then_some(Tone::Neutral),
                unit: if picked == 0 {
                    "pick below".into()
                } else {
                    String::new()
                },
                ..Hop::default()
            },
        ],
        edges: vec![
            added(if visitor { "will visit" } else { "will dial" }),
            added(if visitor { "may use" } else { "serves" }),
        ],
        summary: format!("A new {} for this server", singular(section)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singular_names() {
        assert_eq!(singular("clients"), "client");
        assert_eq!(singular("services"), "service");
    }

    fn heads(d: &burrow_ui_kit::PathData) -> Vec<(String, String)> {
        d.hops
            .iter()
            .map(|h| (h.head.clone(), h.value.clone()))
            .collect()
    }

    #[test]
    fn a_new_service_path_runs_from_who_connects_to_its_target() {
        let d = new_path("services", "", "127.0.0.1", 0);
        assert_eq!(
            heads(&d),
            [
                ("Who connects".to_string(), "public or visitors".to_string()),
                ("Server".to_string(), "new service".to_string()),
                (
                    "Client".to_string(),
                    burrow_admin_logic::wording::NOT_SET.to_string()
                ),
                (
                    "Target".to_string(),
                    burrow_admin_logic::wording::NOT_SET.to_string()
                ),
            ]
        );
        assert_eq!(d.edges.len(), 3);
        assert!(
            d.edges.iter().all(|e| e.above.is_empty()),
            "a service neither dials nor serves"
        );
        assert!(
            d.hops.iter().all(|h| h.unit != "pick below"),
            "the service form has no picker"
        );
        assert!(
            d.hops.iter().all(|h| h.dot == Some(Tone::Neutral)),
            "nothing on a new service is set or online yet"
        );
        assert_eq!(heads(&new_path("services", "web", "h", 0))[1].1, "web");
    }

    #[test]
    fn a_new_client_or_visitor_path_runs_through_this_server_to_its_services() {
        let c = new_path("clients", "", "127.0.0.1", 0);
        assert_eq!(
            heads(&c),
            [
                ("CLIENT".to_string(), "new client".to_string()),
                ("Server".to_string(), "127.0.0.1".to_string()),
                ("Services".to_string(), "none".to_string()),
            ]
        );
        assert_eq!(c.edges[0].above, "will dial");
        assert_eq!(c.edges[1].above, "serves");
        let v = new_path("visitors", "phone", "127.0.0.1", 2);
        assert_eq!(heads(&v)[0], ("VISITOR".to_string(), "phone".to_string()));
        assert_eq!(c.hops[2].dot, Some(Tone::Neutral), "no service picked yet");
        assert_eq!(heads(&v)[2].1, "2 services");
        assert_eq!(v.hops[2].dot, None);
        assert_eq!(v.edges[0].above, "will visit");
        assert_eq!(v.edges[1].above, "may use");
    }
}
