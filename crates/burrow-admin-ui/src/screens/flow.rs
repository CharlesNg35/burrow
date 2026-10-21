use burrow_admin_logic::rates::{Rates, human_bits};
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{ClientEntry, Entry, Protocol, ServiceEntry, VisitorEntry};
use burrow_ui_kit::{Icon, Segmented, active_element, button_class, focus_first, trap_focus};
use leptos::html;
use leptos::prelude::*;

use crate::form::plain;
use crate::format;
use crate::route::{self, Route};
use crate::screens::paths::clients_of;
use crate::store::store;

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

pub fn layout(doc: &Value, status: Option<&Status>, rates: &Rates) -> Chart {
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
    let client_names: Vec<String> = ClientEntry::names(doc);
    let rows = svcs.len().max(client_names.len()).max(2);
    let height = TOP + rows as f64 * PITCH;
    let row_y = |i: usize| TOP + i as f64 * PITCH + 6.0;
    let mid = |n: &Node| n.y + n.h / 2.0;
    let mut nodes = Vec::new();
    let mut lines = Vec::new();
    let public: Vec<usize> = (0..svcs.len())
        .filter(|i| svcs[*i].bind.is_some())
        .collect();
    let private: Vec<usize> = (0..svcs.len())
        .filter(|i| svcs[*i].bind.is_none())
        .collect();
    let span = |idx: &[usize]| {
        let a = row_y(*idx.first().unwrap_or(&0));
        let b = row_y(*idx.last().unwrap_or(&0)) + 42.0;
        (a, (b - a).max(42.0))
    };
    let offline: Vec<&String> = client_names
        .iter()
        .filter(|c| {
            status.is_some()
                && !status
                    .and_then(|s| s.clients.iter().find(|x| &x.name == *c))
                    .is_some_and(|x| x.connected)
        })
        .collect();
    let problem_svc = |s: &Svc| matches!(s.health, Health::Down | Health::Degraded);
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
                "{} public · {lo} loopback · {} refused",
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
            .map(|s| s.visitors.iter().filter(|v| v.open > 0).count())
            .unwrap_or(0);
        nodes.push(Node {
            key: VisitorEntry::section().into(),
            col: 0,
            y,
            h,
            title: "Visitors".into(),
            right: String::new(),
            sub: format!("{active} active"),
            badge: None,
            health: Health::Unknown,
            dashed: false,
            server: false,
            problem: private.iter().any(|i| problem_svc(&svcs[*i])),
            to: Some(Route::List(VisitorEntry::section().into())),
            tip: "Open visitors →".into(),
        });
    }
    let total_open: u64 = svcs.iter().map(|s| s.open).sum();
    let (rate, unit) = human_bits(rates.total.bytes_in);
    let server = Node {
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
    };
    let server_left = COLS[2].0;
    let server_right = COLS[2].0 + COLS[2].1;
    nodes.push(server);
    let client_nodes: Vec<Node> = client_names
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
                    Some(x) => {
                        format::left(x.left_secs_ago, status.map(|s| s.uptime_secs).unwrap_or(0))
                    }
                    None => format::left(None, status.map(|s| s.uptime_secs).unwrap_or(0)),
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
        .collect();
    for (i, s) in svcs.iter().enumerate() {
        let y = row_y(i);
        let problem = problem_svc(s);
        let port = Node {
            key: format!("port:{}", s.name),
            col: 1,
            y,
            h: 42.0,
            title: s.name.clone(),
            right: s
                .bind
                .as_deref()
                .map(short_bind)
                .unwrap_or_else(|| "private".into()),
            sub: format!("{} · {} open", s.protocol, s.open),
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
            tip: format!("Open service {} →", s.name),
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
        nodes.push(port);
        nodes.push(target);
    }
    for cn in &client_nodes {
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

fn node_view(
    n: Node,
    only_problems: Signal<bool>,
    hover: RwSignal<Option<(String, f64, f64)>>,
    wrap: NodeRef<html::Div>,
) -> impl IntoView {
    let s = store();
    let (x, w) = COLS[n.col];
    let y = n.y;
    let faded = n.problem;
    let opacity = move || {
        if only_problems.get() && !faded {
            "0.28"
        } else {
            "1"
        }
    };
    let rect = if n.server {
        "fill-accent-weak stroke-accent".to_string()
    } else {
        n.health.node().to_string()
    };
    let to = n.to.clone();
    let tip = n.tip.clone();
    let lines: Vec<String> = n.sub.lines().map(str::to_string).collect();
    let cursor = if to.is_some() {
        "group cursor-pointer"
    } else {
        ""
    };
    let badge = n.badge.clone().map(|b| {
        let tone = match n.health {
            Health::Down => "fill-danger",
            Health::Degraded => "fill-warning",
            _ => "fill-text-muted",
        };
        view! { <text x=x + w - 8.0 y=y + 30.0 text-anchor="end" class=format!("text-[10.5px] font-semibold {tone}")>{b}</text> }
    });
    let dot = (!n.server && n.col == 3 && !n.dashed && n.health == Health::Healthy)
        .then(|| view! { <circle cx=x + 12.0 cy=y + 15.0 r="3.5" class="fill-success"></circle> });
    let tx = if dot.is_some() { x + 21.0 } else { x + 10.0 };
    view! {
        <g
            class=cursor
            opacity=opacity
            data-node=n.key.clone()
            on:click=move |_| {
                if let Some(r) = to.clone() {
                    s.flow_open.set(false);
                    route::go(r);
                }
            }
            on:mousemove=move |e| {
                if tip.is_empty() {
                    return;
                }
                if let Some(el) = wrap.get_untracked() {
                    let r = el.get_bounding_client_rect();
                    hover.set(Some((tip.clone(), e.client_x() as f64 - r.left() + el.scroll_left() as f64 + 12.0, e.client_y() as f64 - r.top() + el.scroll_top() as f64 + 14.0)));
                }
            }
            on:mouseleave=move |_| hover.set(None)
        >
            <rect
                x=x
                y=y
                width=w
                height=n.h
                rx=if n.server { "12" } else { "9" }
                class=format!("{rect} group-hover:stroke-accent group-hover:stroke-2")
                stroke-width=if n.server { "1.5" } else { "1" }
                stroke-dasharray=n.dashed.then_some("4 3")
            ></rect>
            {dot}
            <text x=tx y=y + 17.0 class=if n.col == 4 { "fill-text font-mono text-[11.5px] font-semibold" } else { "fill-text text-[12px] font-semibold" }>{n.title.clone()}</text>
            {(!n.right.is_empty()).then(|| view! { <text x=x + w - 8.0 y=y + 17.0 text-anchor="end" class="fill-text-muted font-mono text-[11px]">{n.right.clone()}</text> })}
            {lines.into_iter().enumerate().map(|(i, l)| view! {
                <text x=x + 10.0 y=y + 31.0 + i as f64 * 14.0 class="fill-text-muted text-[11px]">{l}</text>
            }).collect_view()}
            {badge}
        </g>
    }
}

#[component]
pub fn FlowModal() -> impl IntoView {
    let s = store();
    let only_problems = RwSignal::new(false);
    let chart = Memo::new(move |_| {
        if !s.flow_open.get() {
            return Chart::default();
        }
        let doc = s.running();
        let status = s.status.get();
        let rates = s.rates.get();
        layout(&doc, status.as_deref(), &rates)
    });
    let panel: NodeRef<html::Div> = NodeRef::new();
    let wrap: NodeRef<html::Div> = NodeRef::new();
    let hover: RwSignal<Option<(String, f64, f64)>> = RwSignal::new(None);
    let back: StoredValue<Option<web_sys::HtmlElement>, LocalStorage> =
        StoredValue::new_local(None);
    Effect::new(move |was: Option<bool>| {
        let open = s.flow_open.get();
        if open && was != Some(true) {
            back.set_value(active_element());
            let c = chart.get_untracked();
            only_problems.set(c.services > 12 && c.problems > 0);
            if let Some(p) = panel.get_untracked() {
                focus_first(&p);
            }
        } else if !open && was == Some(true) {
            if let Some(b) = back.get_value() {
                let _ = b.focus();
            }
            back.set_value(None);
            hover.set(None);
        }
        open
    });
    let close = move || s.flow_open.set(false);
    let filter = Signal::derive(move || {
        if only_problems.get() {
            "problems".to_string()
        } else {
            "all".to_string()
        }
    });
    let svg = move || {
        let c = chart.get();
        let h = c.height;
        let op = Signal::from(only_problems);
        let labels = LABELS
            .iter()
            .zip(COLS)
            .map(|(l, (x, _))| view! { <text x=x y="12" class="fill-text-faint text-[10.5px] font-semibold tracking-[0.06em] uppercase">{*l}</text> })
            .collect_view();
        let lines = c
            .lines
            .into_iter()
            .map(|l| {
                let p = l.problem;
                view! {
                    <path
                        d=l.d
                        fill="none"
                        class=l.stroke
                        stroke-width=format!("{:.2}", l.width)
                        stroke-linecap="round"
                        stroke-dasharray=l.dashed.then_some("4 4")
                        opacity=move || if op.get() && !p { "0.07" } else { "1" }
                    ></path>
                }
            })
            .collect_view();
        let nodes = c
            .nodes
            .into_iter()
            .map(|n| node_view(n, op, hover, wrap))
            .collect_view();
        view! {
            <svg viewBox=format!("0 0 {WIDTH} {h}") width="100%" class="block min-w-[1000px]" role="img" aria-label=c.alt data-testid="flow-svg">
                {labels}
                <g>{lines}</g>
                {nodes}
            </svg>
        }
    };
    let links = move || {
        let c = chart.get();
        c.nodes
            .into_iter()
            .filter_map(|n| n.to.map(|to| (n.tip, to)))
            .map(|(tip, to)| {
                let label = tip.trim_end_matches(" →").to_string();
                view! {
                    <li>
                        <a href=crate::route::href(&to) on:click=move |e| {
                            e.prevent_default();
                            s.flow_open.set(false);
                            route::go(to.clone());
                        }>{label}</a>
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <Show when=move || s.flow_open.get()>
            <div class="fixed inset-0 z-40 bg-scrim" on:click=move |_| close()></div>
            <div
                node_ref=panel
                class="fixed top-14 right-24 left-24 z-50 flex max-h-[calc(100vh-80px)] flex-col rounded-[16px] border border-border-strong bg-surface shadow-overlay max-lg:right-4 max-lg:left-4 max-sm:inset-0 max-sm:max-h-none max-sm:rounded-none dark:bg-surface-2"
                role="dialog"
                aria-modal="true"
                aria-label="Flow"
                data-testid="flow"
                on:keydown=move |e| {
                    if e.key() == "Escape" {
                        e.stop_propagation();
                        close();
                    }
                    if let Some(p) = panel.get_untracked() {
                        trap_focus(&e, &p);
                    }
                }
            >
                <div class="flex flex-wrap items-center gap-3 border-b border-border px-5 py-3.5">
                    <span class="text-accent-ink"><Icon name="network"/></span>
                    <h2 class="text-[17px] font-bold">"Flow"</h2>
                    <span class="text-sm text-text-muted">"live · every connection from source to target"</span>
                    <span class="ml-auto"></span>
                    {move || {
                        let n = chart.with(|c| c.problems);
                        view! {
                            <Segmented
                                name="flow-filter"
                                label="Show"
                                options=vec![("all".to_string(), "All flows".to_string(), false), ("problems".to_string(), format!("Problems {n}"), false)]
                                value=filter
                                on_change=Callback::new(move |v: String| only_problems.set(v == "problems"))
                            />
                        }
                    }}
                    <button type="button" class=button_class("ghost", true) aria-label="Close" data-testid="flow-close" on:click=move |_| close()>
                        <Icon name="x"/>
                    </button>
                </div>
                <div node_ref=wrap class="relative min-h-0 flex-1 overflow-auto overscroll-contain px-[22px] pt-4 pb-1.5">
                    {svg}
                    {move || hover.get().map(|(t, x, y)| view! {
                        <div class="pointer-events-none absolute z-10 rounded-[6px] bg-tip px-2 py-1 text-xs whitespace-nowrap text-white" style:left=format!("{x:.0}px") style:top=format!("{y:.0}px")>{t}</div>
                    })}
                    <ul class="sr-only" aria-label="Open from the flow chart">{links}</ul>
                </div>
                <div class="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border px-4 py-2.5 text-xs text-text-muted">
                    <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-success"></span>"healthy"</span>
                    <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-warning"></span>"degraded"</span>
                    <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-danger"></span>"down"</span>
                    <span class="flex items-center gap-1.5"><span class="w-4 border-t border-dashed border-text-faint"></span>"offline"</span>
                    <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-accent"></span>"server ↔ client channels"</span>
                    <span>"line weight = open connections; client → target lines are fixed"</span>
                    <span class="ml-auto">"Click a node to open that service or client · Esc closes"</span>
                </div>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let c = layout(&doc, Some(&st), &Rates::default());
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
    fn the_flow_marks_down_services_and_away_clients_as_problems() {
        let doc = json!({
            "clients": {"a": {}, "b": {}},
            "services": {
                "web": {"client": "a", "bind": "0.0.0.0:443", "target": "x:80"},
                "db": {"client": "b", "target": "y:5432"},
                "lo": {"client": "a", "bind": "127.0.0.1:3000", "target": "z:3000"}
            }
        });
        let c = layout(&doc, Some(&status()), &Rates::default());
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
        assert_eq!(internet.sub, "1 public · 1 loopback · 0 refused");
        let db = c.nodes.iter().find(|n| n.key == "target:db").unwrap();
        assert_eq!(db.health, Health::Down);
        assert_eq!(db.sub, "no client online");
        let b = c.nodes.iter().find(|n| n.key == "client:b").unwrap();
        assert!(b.dashed && b.problem);
        assert_eq!(b.badge.as_deref(), Some("away"));
        assert_eq!(
            c.nodes.iter().find(|n| n.key == "port:web").unwrap().right,
            ":443"
        );
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
        let c = layout(&doc, None, &Rates::default());
        assert_eq!(c.problems, 0);
        assert!(c.nodes.iter().all(|n| !n.problem));
    }
}
