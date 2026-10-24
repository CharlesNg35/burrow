use burrow_admin_logic::rates::{Rates, human_bits};
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{ClientEntry, Entry, Protocol, ServiceEntry, VisitorEntry};

use crate::form::plain;
use crate::format;
use crate::route::Route;
use crate::screens::paths::clients_of;

use super::links::{client_lines, service_nodes};
use super::{Health, PITCH, TOP};

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub key: String,
    pub col: usize,
    pub y: f64,
    pub h: f64,
    pub title: String,
    pub right: String,
    pub sub: String,
    pub badge: Option<String>,
    pub health: Health,
    pub dashed: bool,
    pub server: bool,
    pub problem: bool,
    pub to: Option<Route>,
    pub tip: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub d: String,
    pub width: f64,
    pub stroke: &'static str,
    pub dashed: bool,
    pub problem: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chart {
    pub nodes: Vec<Node>,
    pub lines: Vec<Line>,
    pub height: f64,
    pub problems: usize,
    pub services: usize,
    pub alt: String,
}

pub(super) fn weight(open: u64) -> f64 {
    1.4 + open.min(50) as f64 * 0.14
}

fn loopback(bind: &str) -> bool {
    bind.starts_with("127.") || bind.starts_with("[::1]") || bind.starts_with("localhost")
}

pub(super) fn short_bind(bind: &str) -> String {
    match bind
        .strip_prefix("0.0.0.0")
        .or_else(|| bind.strip_prefix("[::]"))
    {
        Some(rest) => rest.to_string(),
        None => bind.to_string(),
    }
}

pub(super) struct Svc {
    pub(super) name: String,
    pub(super) bind: Option<String>,
    pub(super) target: String,
    pub(super) protocol: String,
    pub(super) clients: Vec<String>,
    pub(super) open: u64,
    pub(super) rate: f64,
    pub(super) health: Health,
    pub(super) badge: Option<String>,
}

pub fn layout(doc: &Value, status: Option<&Status>, rates: &Rates, me: Option<&str>) -> Chart {
    let svcs = services(doc, status, rates);
    let client_names: Vec<String> = ClientEntry::names(doc);
    let rows = svcs.len().max(client_names.len()).max(2);
    let height = TOP + rows as f64 * PITCH;
    let mut nodes = sources(&svcs, status, me);
    let mut lines = Vec::new();
    let offline: Vec<&String> = client_names
        .iter()
        .filter(|c| {
            status.is_some()
                && !status
                    .and_then(|s| s.clients.iter().find(|x| &x.name == *c))
                    .is_some_and(|x| x.connected)
        })
        .collect();
    nodes.push(server_node(&svcs, status, rates, rows));
    let client_nodes = client_nodes(&client_names, &svcs, status, rows);
    for (i, s) in svcs.iter().enumerate() {
        let (port, target) = service_nodes(i, s, &nodes, &client_nodes, status, &mut lines);
        nodes.push(port);
        nodes.push(target);
    }
    client_lines(&client_nodes, status, &mut lines);
    let problems = svcs.iter().filter(|s| problem_svc(s)).count() + offline.len();
    nodes.extend(client_nodes);
    let alt = format!(
        "Flow of {} services through the server to {} clients; {} problems",
        svcs.len(),
        client_names.len(),
        problems
    );
    Chart {
        nodes,
        lines,
        height,
        problems,
        services: svcs.len(),
        alt,
    }
}

pub(super) fn row_y(i: usize) -> f64 {
    TOP + i as f64 * PITCH + 6.0
}

fn span(idx: &[usize]) -> (f64, f64) {
    let a = row_y(*idx.first().unwrap_or(&0));
    let b = row_y(*idx.last().unwrap_or(&0)) + 42.0;
    (a, (b - a).max(42.0))
}

pub(super) fn problem_svc(s: &Svc) -> bool {
    matches!(s.health, Health::Down | Health::Degraded)
}

fn services(doc: &Value, status: Option<&Status>, rates: &Rates) -> Vec<Svc> {
    let mut svcs: Vec<Svc> = ServiceEntry::entries(doc)
        .map(|m| {
            m.iter()
                .map(|(name, v)| {
                    let st = status.and_then(|s| s.services.iter().find(|x| &x.name == name));
                    let clients = clients_of(v);
                    let up = clients
                        .iter()
                        .filter(|c| {
                            status
                                .and_then(|s| s.clients.iter().find(|x| &x.name == *c))
                                .is_some_and(|x| x.connected)
                        })
                        .count();
                    let checks = st
                        .map(|x| (x.health.iter().filter(|h| h.up).count(), x.health.len()))
                        .unwrap_or((0, 0));
                    let (health, badge) = if status.is_none() {
                        (Health::Unknown, None)
                    } else if up == 0 {
                        (Health::Down, Some("down".to_string()))
                    } else if checks.1 > 0 && checks.0 < checks.1 {
                        (
                            Health::Degraded,
                            Some(format!("{} of {} up", checks.0, checks.1)),
                        )
                    } else if up < clients.len() {
                        (
                            Health::Degraded,
                            Some(format!("{up} of {} up", clients.len())),
                        )
                    } else {
                        (Health::Healthy, None)
                    };
                    Svc {
                        name: name.clone(),
                        bind: ServiceEntry::BIND
                            .get(v)
                            .filter(|b| !b.is_null())
                            .map(plain),
                        target: ServiceEntry::TARGET.get(v).map(plain).unwrap_or_default(),
                        protocol: ServiceEntry::PROTOCOL
                            .get(v)
                            .map(plain)
                            .unwrap_or_else(|| Protocol::default().wire().into()),
                        clients,
                        open: st.map(|x| x.open).unwrap_or(0),
                        rate: rates.service(name).bytes_in,
                        health,
                        badge,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    svcs.sort_by(|a, b| {
        a.bind
            .is_none()
            .cmp(&b.bind.is_none())
            .then_with(|| a.name.cmp(&b.name))
    });
    svcs
}

fn sources(svcs: &[Svc], status: Option<&Status>, me: Option<&str>) -> Vec<Node> {
    let mut nodes = Vec::new();
    let public: Vec<usize> = (0..svcs.len())
        .filter(|i| svcs[*i].bind.is_some())
        .collect();
    let private: Vec<usize> = (0..svcs.len())
        .filter(|i| svcs[*i].bind.is_none())
        .collect();
    if !public.is_empty() {
        let lo = public
            .iter()
            .filter(|i| svcs[**i].bind.as_deref().is_some_and(loopback))
            .count();
        let (y, h) = span(&public);
        nodes.push(Node {
            key: "internet".into(),
            col: 0,
            y,
            h,
            title: "Internet".into(),
            right: String::new(),
            sub: format!(
                "{} public · {lo} loopback\n{} refused",
                public.len() - lo,
                status
                    .map(|s| s.services.iter().map(|x| x.refused).sum::<u64>())
                    .unwrap_or(0)
            ),
            badge: None,
            health: Health::Unknown,
            dashed: false,
            server: false,
            problem: public.iter().any(|i| problem_svc(&svcs[*i])),
            to: None,
            tip: String::new(),
        });
    }
    if !private.is_empty() {
        let (y, h) = span(&private);
        let active = status
            .map(|s| {
                s.visitors
                    .iter()
                    .filter(|v| v.open > 0 || me == Some(v.name.as_str()))
                    .count()
            })
            .unwrap_or(0);
        nodes.push(Node {
            key: VisitorEntry::section().into(),
            col: 0,
            y,
            h,
            title: "Visitors".into(),
            right: String::new(),
            sub: format!("{active} in use"),
            badge: None,
            health: Health::Unknown,
            dashed: false,
            server: false,
            problem: private.iter().any(|i| problem_svc(&svcs[*i])),
            to: Some(Route::List(VisitorEntry::section().into())),
            tip: "Open visitors →".into(),
        });
    }
    nodes
}

fn server_node(svcs: &[Svc], status: Option<&Status>, rates: &Rates, rows: usize) -> Node {
    let total_open: u64 = svcs.iter().map(|s| s.open).sum();
    let (rate, unit) = human_bits(rates.total.bytes_in);
    Node {
        key: "server".into(),
        col: 2,
        y: TOP + 6.0,
        h: rows as f64 * PITCH - 12.0,
        title: "this server".into(),
        right: String::new(),
        sub: format!(
            "{}\n{total_open} open\n{rate} {unit} in\n{} UDP peers",
            status
                .map(|s| s
                    .listening
                    .iter()
                    .map(|l| short_bind(&format::listen_addr(l)))
                    .collect::<Vec<_>>()
                    .join(" · "))
                .unwrap_or_default(),
            status.map(|s| s.udp_peers).unwrap_or(0)
        ),
        badge: None,
        health: Health::Unknown,
        dashed: false,
        server: true,
        problem: false,
        to: None,
        tip: String::new(),
    }
}

fn client_nodes(
    client_names: &[String],
    svcs: &[Svc],
    status: Option<&Status>,
    rows: usize,
) -> Vec<Node> {
    client_names
        .iter()
        .enumerate()
        .map(|(j, c)| {
            let cs = status.and_then(|s| s.clients.iter().find(|x| &x.name == c));
            let on = cs.is_some_and(|x| x.connected);
            let slot = (rows as f64 * PITCH) / client_names.len().max(1) as f64;
            let y = TOP + (j as f64 + 0.5) * slot - 21.0;
            let involved = svcs.iter().any(|s| problem_svc(s) && s.clients.contains(c));
            Node {
                key: format!("client:{c}"),
                col: 3,
                y,
                h: 42.0,
                title: c.clone(),
                right: String::new(),
                sub: match cs {
                    Some(x) if x.connected => format!("{} open · {} idle", x.open, x.idle_channels),
                    Some(x) => match x.left_secs_ago {
                        Some(a) => format!("left {}", format::ago(a)),
                        None => "not seen since start".into(),
                    },
                    None => "not seen since start".into(),
                },
                badge: (status.is_some() && !on)
                    .then(|| format::away_badge(cs.and_then(|x| x.left_secs_ago)).to_string()),
                health: if status.is_none() {
                    Health::Unknown
                } else if on {
                    Health::Healthy
                } else {
                    Health::Offline
                },
                dashed: status.is_some() && !on,
                server: false,
                problem: involved || (status.is_some() && !on),
                to: Some(Route::Detail(ClientEntry::section().into(), c.clone())),
                tip: format!("Open client {c} →"),
            }
        })
        .collect()
}
