mod view;

use burrow_admin_logic::rates::{Rates, human_bits};
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{ClientEntry, Entry, Protocol, ServiceEntry, VisitorEntry};

use crate::form::plain;
use crate::format;
use crate::route::Route;
use crate::screens::paths::clients_of;

pub(crate) use view::FlowModal;

pub const PITCH: f64 = 54.0;
pub const TOP: f64 = 26.0;
pub const WIDTH: f64 = 1080.0;
const COLS: [(f64, f64); 5] = [
    (0.0, 180.0),
    (220.0, 190.0),
    (450.0, 140.0),
    (630.0, 190.0),
    (860.0, 220.0),
];
const LABELS: [&str; 5] = ["Sources", "Public ports", "Server", "Clients", "Targets"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Healthy,
    Degraded,
    Down,
    Offline,
    Unknown,
}

impl Health {
    fn stroke(self) -> &'static str {
        match self {
            Health::Healthy => "stroke-success/55",
            Health::Degraded => "stroke-warning/80",
            Health::Down => "stroke-danger/85",
            Health::Offline | Health::Unknown => "stroke-text-faint",
        }
    }

    fn node(self) -> &'static str {
        match self {
            Health::Degraded => "fill-warning-weak stroke-warning",
            Health::Down => "fill-danger-weak stroke-danger",
            _ => "fill-surface stroke-border-strong dark:fill-surface-3",
        }
    }
}

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

fn weight(open: u64) -> f64 {
    1.4 + open.min(50) as f64 * 0.14
}

fn curve(x1: f64, y1: f64, x2: f64, y2: f64) -> String {
    let mx = (x1 + x2) / 2.0;
    format!("M{x1:.1},{y1:.1} C{mx:.1},{y1:.1} {mx:.1},{y2:.1} {x2:.1},{y2:.1}")
}

fn loopback(bind: &str) -> bool {
    bind.starts_with("127.") || bind.starts_with("[::1]") || bind.starts_with("localhost")
}

fn short_bind(bind: &str) -> String {
    match bind
        .strip_prefix("0.0.0.0")
        .or_else(|| bind.strip_prefix("[::]"))
    {
        Some(rest) => rest.to_string(),
        None => bind.to_string(),
    }
}

struct Svc {
    name: String,
    bind: Option<String>,
    target: String,
    protocol: String,
    clients: Vec<String>,
    open: u64,
    rate: f64,
    health: Health,
    badge: Option<String>,
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

fn row_y(i: usize) -> f64 {
    TOP + i as f64 * PITCH + 6.0
}

fn mid(n: &Node) -> f64 {
    n.y + n.h / 2.0
}

fn span(idx: &[usize]) -> (f64, f64) {
    let a = row_y(*idx.first().unwrap_or(&0));
    let b = row_y(*idx.last().unwrap_or(&0)) + 42.0;
    (a, (b - a).max(42.0))
}

fn problem_svc(s: &Svc) -> bool {
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

fn service_nodes(
    i: usize,
    s: &Svc,
    nodes: &[Node],
    client_nodes: &[Node],
    status: Option<&Status>,
    lines: &mut Vec<Line>,
) -> (Node, Node) {
    let server_left = COLS[2].0;
    let server_right = COLS[2].0 + COLS[2].1;
    let y = row_y(i);
    let problem = problem_svc(s);
    let port = Node {
        key: format!("port:{}", s.name),
        col: 1,
        y,
        h: 42.0,
        title: s.name.clone(),
        right: String::new(),
        sub: format!(
            "{} · {} · {} open",
            s.bind
                .as_deref()
                .map(short_bind)
                .unwrap_or_else(|| "private".into()),
            s.protocol,
            s.open
        ),
        badge: None,
        health: s.health,
        dashed: false,
        server: false,
        problem,
        to: Some(Route::Detail(
            ServiceEntry::section().into(),
            s.name.clone(),
        )),
        tip: format!("Open service {} →", s.name),
    };
    let (r, u) = human_bits(s.rate);
    let target = Node {
        key: format!("target:{}", s.name),
        col: 4,
        y,
        h: 42.0,
        title: s.target.clone(),
        right: String::new(),
        sub: if s.health == Health::Down {
            "no client online".into()
        } else {
            format!("{} open · {r} {u}", s.open)
        },
        badge: s.badge.clone(),
        health: s.health,
        dashed: false,
        server: false,
        problem,
        to: Some(Route::Detail(
            ServiceEntry::section().into(),
            s.name.clone(),
        )),
        tip: format!("{} · open service {} →", s.target, s.name),
    };
    let w = weight(s.open);
    let stroke = s.health.stroke();
    if let Some(src) = nodes.iter().find(|n| {
        n.col == 0
            && n.key
                == if s.bind.is_some() {
                    "internet"
                } else {
                    VisitorEntry::section()
                }
    }) {
        lines.push(Line {
            d: curve(COLS[0].0 + COLS[0].1, mid(src), COLS[1].0, mid(&port)),
            width: w,
            stroke,
            dashed: false,
            problem,
        });
    }
    lines.push(Line {
        d: curve(COLS[1].0 + COLS[1].1, mid(&port), server_left, mid(&port)),
        width: w,
        stroke,
        dashed: false,
        problem,
    });
    for c in &s.clients {
        let local = status
            .and_then(|st| st.clients.iter().find(|x| &x.name == c))
            .is_some_and(|x| x.local);
        if local {
            lines.push(Line {
                d: curve(server_right, mid(&port), COLS[4].0, mid(&target)),
                width: w,
                stroke,
                dashed: false,
                problem,
            });
            continue;
        }
        if let Some(cn) = client_nodes.iter().find(|n| n.key == format!("client:{c}")) {
            let off = cn.dashed;
            lines.push(Line {
                d: curve(COLS[3].0 + COLS[3].1, mid(cn), COLS[4].0, mid(&target)),
                width: 1.4,
                stroke: if off { "stroke-text-faint" } else { stroke },
                dashed: off,
                problem: problem || off,
            });
        }
    }
    (port, target)
}

fn client_lines(client_nodes: &[Node], status: Option<&Status>, lines: &mut Vec<Line>) {
    let server_right = COLS[2].0 + COLS[2].1;
    for cn in client_nodes {
        let open = status
            .and_then(|st| {
                st.clients
                    .iter()
                    .find(|x| format!("client:{}", x.name) == cn.key)
            })
            .map(|x| x.open)
            .unwrap_or(0);
        lines.push(Line {
            d: curve(server_right, mid(cn), COLS[3].0, mid(cn)),
            width: weight(open),
            stroke: if cn.dashed {
                "stroke-text-faint"
            } else {
                "stroke-accent/60"
            },
            dashed: cn.dashed,
            problem: cn.problem,
        });
    }
}

fn fit(text: &str, room: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= room {
        return text.to_string();
    }
    let keep = room.saturating_sub(1);
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_cut_in_the_middle() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("unix:/home/ubuntu/app.sock", 12), "unix:/….sock");
        assert_eq!(fit("unix:/home/ubuntu/app.sock", 12).chars().count(), 12);
    }
    use burrow_admin_proto::serde_json::json;
    use burrow_admin_proto::status::{ClientStatus, ServiceStatus};

    fn status() -> Status {
        let mut st: Status = burrow_admin_proto::serde_json::from_value(json!({
            "version": "0.1.0", "uptime_secs": 10, "listening": ["0.0.0.0:7835"], "handshakes_refused": 0, "udp_peers": 0,
            "clients": [], "services": [], "visitors": []
        }))
        .unwrap();
        st.clients.push(ClientStatus {
            name: "a".into(),
            connected: true,
            connected_secs: Some(5),
            from: Some("1.2.3.4".into()),
            open: 3,
            idle_channels: 2,
            left_secs_ago: None,
            local: false,
            ..ClientStatus::default()
        });
        st.clients.push(ClientStatus {
            name: "b".into(),
            connected: false,
            connected_secs: None,
            from: None,
            open: 0,
            idle_channels: 0,
            left_secs_ago: Some(7200),
            local: false,
            ..ClientStatus::default()
        });
        let mut svc: ServiceStatus = burrow_admin_proto::serde_json::from_value(
            json!({"name": "web", "protocol": "tcp", "target": "x:80", "clients": ["a"]}),
        )
        .unwrap();
        svc.open = 60;
        st.services.push(svc);
        st
    }

    #[test]
    fn a_service_on_this_server_goes_from_the_server_straight_to_its_target() {
        let mut st = status();
        st.clients.push(ClientStatus {
            name: "self".into(),
            connected: true,
            local: true,
            ..ClientStatus::default()
        });
        let doc = json!({
            "clients": {},
            "services": {"own": {"client": "self", "bind": "0.0.0.0:8080", "target": "127.0.0.1:3000"}}
        });
        let c = layout(&doc, Some(&st), &Rates::default(), None);
        assert_eq!(c.problems, 0);
        let target = c.nodes.iter().find(|n| n.key == "target:own").unwrap();
        assert_eq!(target.health, Health::Healthy);
        assert!(c.nodes.iter().all(|n| n.key != "client:self"));
        assert_eq!(c.lines.len(), 3, "source, port to server, server to target");
        let direct = &c.lines[2].d;
        assert!(
            direct.contains(&format!(" {:.1},", COLS[4].0)),
            "the line ends at the target column: {direct}"
        );
    }

    #[test]
    fn the_source_nodes_say_their_counts_without_cutting_them() {
        let doc = json!({
            "clients": {"a": {}},
            "visitors": {"v": {}},
            "services": {
                "web": {"client": "a", "bind": "0.0.0.0:443", "target": "x:80"},
                "lo": {"client": "a", "bind": "127.0.0.1:3000", "target": "z:3000"},
                "db": {"client": "a", "target": "y:5432", "visitors": ["v"]}
            }
        });
        let mut st = status();
        st.visitors.push(
            burrow_admin_proto::serde_json::from_value(json!({"name": "v", "open": 0})).unwrap(),
        );
        let c = layout(&doc, Some(&st), &Rates::default(), Some("v"));
        for n in c.nodes.iter().filter(|n| n.col == 0) {
            let room = ((COLS[0].1 - 18.0) / 5.4) as usize;
            for line in n.sub.lines() {
                assert!(line.chars().count() <= room, "{line:?} is cut");
            }
        }
        let visitors = c.nodes.iter().find(|n| n.title == "Visitors").unwrap();
        assert_eq!(
            visitors.sub, "1 in use",
            "your own admin session counts, as on the overview"
        );
    }

    #[test]
    fn the_flow_marks_down_services_and_away_clients_as_problems() {
        let doc = json!({
            "clients": {"a": {}, "b": {}},
            "services": {
                "web": {"client": "a", "bind": "0.0.0.0:443", "target": "x:80"},
                "db": {"client": "b", "target": "y:5432"},
                "lo": {"client": "a", "bind": "127.0.0.1:3000", "target": "z:3000"}
            }
        });
        let c = layout(&doc, Some(&status()), &Rates::default(), None);
        assert_eq!(c.services, 3);
        assert_eq!(c.problems, 2, "db is down and b is away");
        let order: Vec<&str> = c
            .nodes
            .iter()
            .filter(|n| n.col == 1)
            .map(|n| n.title.as_str())
            .collect();
        assert_eq!(
            order,
            ["lo", "web", "db"],
            "public first, then private, each by name"
        );
        let internet = c.nodes.iter().find(|n| n.key == "internet").unwrap();
        assert_eq!(internet.sub, "1 public · 1 loopback\n0 refused");
        let db = c.nodes.iter().find(|n| n.key == "target:db").unwrap();
        assert_eq!(db.health, Health::Down);
        assert_eq!(db.sub, "no client online");
        let b = c.nodes.iter().find(|n| n.key == "client:b").unwrap();
        assert!(b.dashed && b.problem);
        assert_eq!(b.badge.as_deref(), Some("away"));
        let web = c.nodes.iter().find(|n| n.key == "port:web").unwrap();
        assert!(web.right.is_empty(), "the bind sits on line 2");
        assert!(web.sub.starts_with(":443 · "), "{}", web.sub);
        assert_eq!(c.height, TOP + 3.0 * PITCH);
        assert!(c.lines.iter().all(|l| l.d.starts_with('M')));
    }

    #[test]
    fn line_weight_grows_with_open_connections_and_stops_at_fifty() {
        assert_eq!(weight(0), 1.4);
        assert!((weight(10) - 2.8).abs() < 1e-9);
        assert_eq!(weight(50), weight(500));
    }

    #[test]
    fn without_status_nothing_is_called_a_problem() {
        let doc =
            json!({"clients": {"a": {}}, "services": {"web": {"client": "a", "target": "x:80"}}});
        let c = layout(&doc, None, &Rates::default(), None);
        assert_eq!(c.problems, 0);
        assert!(c.nodes.iter().all(|n| !n.problem));
    }
}
