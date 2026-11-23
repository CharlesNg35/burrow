use burrow_admin_logic::presence::Presence;
use burrow_admin_logic::rates::human_bits;
use burrow_config::{Entry, ServiceEntry, VisitorEntry};

use crate::route::Route;

use super::layout::{Act, Clients, Line, Node, Path, Svc, problem_svc, row_y, short_bind, weight};
use super::live::Live;
use super::{COLS, Health};

fn curve(x1: f64, y1: f64, x2: f64, y2: f64) -> Path {
    let mx = (x1 + x2) / 2.0;
    Path {
        d: format!("M{x1:.1},{y1:.1} C{mx:.1},{y1:.1} {mx:.1},{y2:.1} {x2:.1},{y2:.1}"),
        top: y1.min(y2),
        bottom: y1.max(y2),
    }
}

fn mid(n: &Node) -> f64 {
    n.y + n.h / 2.0
}

pub(super) fn service_nodes(
    i: usize,
    s: &Svc,
    sources: &[Node],
    clients: &Clients,
    live: &Live,
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
                .or_else(|| (!s.web.is_empty()).then(|| s.web.join(", ")))
                .unwrap_or_else(|| s.reached.clone()),
            s.protocol.to_uppercase(),
            s.open
        ),
        badge: None,
        health: s.health,
        dashed: false,
        server: false,
        problem,
        act: Some(Act::Open(Route::Detail(
            ServiceEntry::section().into(),
            s.name.clone(),
        ))),
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
        act: Some(Act::Open(Route::Detail(
            ServiceEntry::section().into(),
            s.name.clone(),
        ))),
        tip: format!("{} · open service {} →", s.target, s.name),
    };
    let w = weight(s.open);
    let stroke = s.health.stroke();
    let from = if s.bind.is_some() {
        "internet"
    } else {
        VisitorEntry::section()
    };
    if let Some(src) = sources.iter().find(|n| n.key == from) {
        lines.push(Line {
            key: format!("src:{}", s.name),
            path: curve(COLS[0].0 + COLS[0].1, mid(src), COLS[1].0, mid(&port)),
            width: w,
            stroke,
            dashed: false,
            problem,
        });
    }
    lines.push(Line {
        key: format!("in:{}", s.name),
        path: curve(COLS[1].0 + COLS[1].1, mid(&port), server_left, mid(&port)),
        width: w,
        stroke,
        dashed: false,
        problem,
    });
    for (j, c) in s.clients.iter().enumerate() {
        let key = format!("to:{}:{j}", s.name);
        if live.presence(c) == Presence::Here {
            lines.push(Line {
                key,
                path: curve(server_right, mid(&port), COLS[4].0, mid(&target)),
                width: w,
                stroke,
                dashed: false,
                problem,
            });
            continue;
        }
        if let Some(cn) = clients.get(c) {
            let off = cn.dashed;
            lines.push(Line {
                key,
                path: curve(COLS[3].0 + COLS[3].1, mid(cn), COLS[4].0, mid(&target)),
                width: 1.4,
                stroke: if off { "stroke-text-faint" } else { stroke },
                dashed: off,
                problem: problem || off,
            });
        }
    }
    (port, target)
}

pub(super) fn client_lines(client_nodes: &[Node], live: &Live, lines: &mut Vec<Line>) {
    let server_right = COLS[2].0 + COLS[2].1;
    for cn in client_nodes {
        let open = live.client(&cn.title).map(|x| x.open).unwrap_or(0);
        lines.push(Line {
            key: format!("ch:{}", cn.title),
            path: curve(server_right, mid(cn), COLS[3].0, mid(cn)),
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
