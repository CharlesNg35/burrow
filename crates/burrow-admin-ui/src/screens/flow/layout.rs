use std::collections::{HashMap, HashSet};

use burrow_admin_logic::rates::{Rates, human_bits};
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{ClientEntry, Entry, Protocol, ServiceEntry, VisitorEntry};

use crate::form::plain;
use crate::format;
use crate::route::Route;
use crate::screens::paths::{clients_of, health_with};
use burrow_admin_logic::health::Level;

use super::bundle::{Kind, Row, dress, header, plan};
use super::links::{client_lines, service_nodes};
use super::live::Live;
use super::place::{Spot, Stick, spots};
use super::{Health, PITCH, TOP};

#[derive(Debug, Clone, PartialEq, Default)]
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
    pub act: Option<Act>,
    pub tip: String,
    pub stick: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Open(Route),
    Fold { client: String, open: bool },
}

impl Act {
    pub fn route(&self) -> Option<&Route> {
        match self {
            Act::Open(r) => Some(r),
            Act::Fold { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    pub d: String,
    pub top: f64,
    pub bottom: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Line {
    pub key: String,
    pub path: Path,
    pub stick: Option<Stick>,
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
    pub total: usize,
    pub alt: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Focus {
    pub query: String,
    pub open: HashSet<String>,
}

pub(super) struct Clients {
    pub(super) nodes: Vec<Node>,
    at: HashMap<String, usize>,
}

impl Clients {
    pub(super) fn get(&self, name: &str) -> Option<&Node> {
        self.at.get(name).and_then(|i| self.nodes.get(*i))
    }
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
    pub(super) web: Vec<String>,
    pub(super) target: String,
    pub(super) protocol: String,
    pub(super) clients: Vec<String>,
    pub(super) open: u64,
    pub(super) rate: f64,
    pub(super) health: Health,
    pub(super) badge: Option<String>,
    pub(super) reached: String,
}

impl Svc {
    fn matches(&self, q: &str) -> bool {
        q.is_empty()
            || self.name.to_lowercase().contains(q)
            || self.target.to_lowercase().contains(q)
            || self.clients.iter().any(|c| c.to_lowercase().contains(q))
    }
}

pub fn layout(
    doc: &Value,
    status: Option<&Status>,
    rates: &Rates,
    me: Option<&str>,
    focus: &Focus,
) -> Chart {
    let live = Live::new(status, rates);
    let q = focus.query.trim().to_lowercase();
    let all = services(doc, &live);
    let total = all.len();
    let svcs: Vec<Svc> = all.into_iter().filter(|s| s.matches(&q)).collect();
    let shown = svcs.len();
    let client_names = shown_clients(ClientEntry::names(doc), &svcs, &q);
    let offline = client_names
        .iter()
        .filter(|c| live.known() && !live.up(c))
        .count();
    let problems = svcs.iter().filter(|s| problem_svc(s)).count() + offline;
    let reach = Reach::of(&svcs);
    let total_open: u64 = svcs.iter().map(|s| s.open).sum();
    let involved: HashSet<String> = svcs
        .iter()
        .filter(|s| problem_svc(s))
        .flat_map(|s| s.clients.iter().cloned())
        .collect();
    let rows = plan(svcs, &focus.open);
    let height_rows = rows.len().max(client_names.len()).max(2);
    let src = sources(&rows, &reach, status, me);
    let server = server_node(total_open, status, rates, height_rows);
    let spots = spots(&client_names, &rows, height_rows, 42.0);
    let clients = client_nodes(&client_names, &involved, &live, &spots);
    let mut lines = Vec::new();
    let mut nodes = Vec::with_capacity(src.len() + 1 + rows.len() * 2 + client_names.len());
    nodes.extend(src.iter().cloned());
    nodes.push(server);
    for (i, r) in rows.iter().enumerate() {
        match &r.kind {
            Kind::Header(g) => nodes.push(header(i, g)),
            kind => {
                let from = lines.len();
                let (mut port, mut target) =
                    service_nodes(i, &r.svc, &src, &clients, &live, &mut lines);
                if let Kind::Bundle(g) = kind {
                    dress(g, &mut port, &mut target, &mut lines[from..]);
                }
                nodes.push(port);
                nodes.push(target);
            }
        }
    }
    client_lines(&clients.nodes, &live, &mut lines);
    nodes.extend(clients.nodes);
    let alt = format!(
        "Flow of {shown} services through the server to {} clients; {problems} problems",
        client_names.len(),
    );
    Chart {
        nodes,
        lines,
        height: TOP + height_rows as f64 * PITCH,
        problems,
        services: shown,
        total,
        alt,
    }
}

struct Reach {
    public: usize,
    local: usize,
}

impl Reach {
    fn of(svcs: &[Svc]) -> Self {
        let public = svcs.iter().filter(|s| s.bind.is_some()).count();
        let local = svcs
            .iter()
            .filter(|s| s.bind.as_deref().is_some_and(loopback))
            .count();
        Reach { public, local }
    }
}

fn shown_clients(names: Vec<String>, svcs: &[Svc], q: &str) -> Vec<String> {
    if q.is_empty() {
        return names;
    }
    let used: HashSet<&str> = svcs
        .iter()
        .flat_map(|s| s.clients.iter().map(String::as_str))
        .collect();
    names
        .into_iter()
        .filter(|c| used.contains(c.as_str()) || c.to_lowercase().contains(q))
        .collect()
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

fn services(doc: &Value, live: &Live) -> Vec<Svc> {
    let mut svcs: Vec<Svc> = ServiceEntry::entries(doc)
        .map(|m| {
            m.iter()
                .map(|(name, v)| {
                    let st = live.service(name);
                    let clients = clients_of(v);
                    let count = live
                        .known()
                        .then(|| health_with(st, v, |c| live.connected(c)));
                    let (health, badge) = match count {
                        None => (Health::Unknown, None),
                        Some(c) => match c.level() {
                            Level::Down => (Health::Down, Some(c.label().to_lowercase())),
                            Level::Partial => (Health::Degraded, Some(c.label())),
                            Level::Up => (Health::Healthy, None),
                        },
                    };
                    Svc {
                        name: name.clone(),
                        bind: ServiceEntry::BIND
                            .get(v)
                            .filter(|b| !b.is_null())
                            .map(plain),
                        web: burrow_admin_logic::web::hosts(v),
                        target: ServiceEntry::TARGET
                            .get(v)
                            .map(burrow_admin_logic::web::target_value_shown)
                            .unwrap_or_default(),
                        protocol: ServiceEntry::PROTOCOL
                            .get(v)
                            .map(plain)
                            .unwrap_or_else(|| Protocol::default().wire().into()),
                        clients,
                        open: st.map(|x| x.open).unwrap_or(0),
                        rate: live.rate_in(name),
                        health,
                        badge,
                        reached: burrow_admin_logic::web::reached(v),
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

fn sources(rows: &[Row], reach: &Reach, status: Option<&Status>, me: Option<&str>) -> Vec<Node> {
    let mut nodes = Vec::new();
    let drawn = |public: bool| -> Vec<usize> {
        (0..rows.len())
            .filter(|i| {
                !matches!(rows[*i].kind, Kind::Header(_)) && rows[*i].svc.bind.is_some() == public
            })
            .collect()
    };
    let public = drawn(true);
    let private = drawn(false);
    let problem = |idx: &[usize]| idx.iter().any(|i| problem_svc(&rows[*i].svc));
    if !public.is_empty() {
        let lo = reach.local;
        let (y, h) = span(&public);
        nodes.push(Node {
            key: "internet".into(),
            col: 0,
            y,
            h,
            title: "Internet".into(),
            right: if lo > 0 {
                format!("{lo} local")
            } else {
                String::new()
            },
            sub: format!(
                "{} public · {} refused",
                reach.public - lo,
                status
                    .map(|s| s.services.iter().map(|x| x.refused).sum::<u64>())
                    .unwrap_or(0)
            ),
            badge: None,
            health: Health::Unknown,
            dashed: false,
            server: false,
            problem: problem(&public),
            act: None,
            tip: String::new(),
            stick: None,
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
            problem: problem(&private),
            act: Some(Act::Open(Route::List(VisitorEntry::section().into()))),
            tip: "Open visitors →".into(),
            stick: None,
        });
    }
    nodes
}

fn server_node(total_open: u64, status: Option<&Status>, rates: &Rates, rows: usize) -> Node {
    let (rate, unit) = human_bits(rates.total.bytes_in);
    Node {
        key: "server".into(),
        col: 2,
        y: TOP + 6.0,
        h: rows as f64 * PITCH - 12.0,
        title: "this server".into(),
        right: String::new(),
        sub: format!(
            "{}\n{total_open} open\n{rate} {unit} in\n{} {}",
            status
                .map(|s| s
                    .listening
                    .iter()
                    .map(|l| short_bind(&format::listen_addr(l)))
                    .collect::<Vec<_>>()
                    .join(" · "))
                .unwrap_or_default(),
            status.map(|s| s.udp_peers).unwrap_or(0),
            burrow_admin_logic::wording::UDP_SENDERS
        ),
        badge: None,
        health: Health::Unknown,
        dashed: false,
        server: true,
        problem: false,
        act: None,
        tip: String::new(),
        stick: None,
    }
}

fn client_nodes(
    client_names: &[String],
    involved: &HashSet<String>,
    live: &Live,
    spots: &[Spot],
) -> Clients {
    let known = live.known();
    let nodes: Vec<Node> = client_names
        .iter()
        .enumerate()
        .map(|(j, c)| {
            let cs = live.client(c);
            let on = live.up(c);
            let spot = spots[j];
            let involved = involved.contains(c);
            Node {
                key: format!("client:{c}"),
                col: 3,
                y: spot.y,
                h: 42.0,
                stick: spot.stick,
                title: c.clone(),
                right: String::new(),
                sub: match cs {
                    Some(x) if x.connected => format!("{} open · {} idle", x.open, x.idle_channels),
                    Some(x) => match x.left_secs_ago {
                        Some(a) => format!("left {}", format::coarse_ago(a)),
                        None => "not seen since start".into(),
                    },
                    None => "not seen since start".into(),
                },
                badge: (known && !on)
                    .then(|| format::away_badge(cs.and_then(|x| x.left_secs_ago)).to_string()),
                health: if !known {
                    Health::Unknown
                } else if on {
                    Health::Healthy
                } else {
                    Health::Offline
                },
                dashed: known && !on,
                server: false,
                problem: involved || (known && !on),
                act: Some(Act::Open(Route::Detail(
                    ClientEntry::section().into(),
                    c.clone(),
                ))),
                tip: format!("Open client {c} →"),
            }
        })
        .collect();
    let at = client_names
        .iter()
        .enumerate()
        .map(|(j, c)| (c.clone(), j))
        .collect();
    Clients { nodes, at }
}
