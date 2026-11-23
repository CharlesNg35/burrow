use std::collections::{HashMap, HashSet};

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::route;
use crate::store::{Store, store};

use super::layout::Line;
use super::place::sticky_y;
use super::{Act, COLS, Chart, Health, LABELS, Node, WIDTH, fit};

const SLACK: f64 = 4.0;

pub(super) type Hover = RwSignal<Option<(String, f64, f64)>>;

#[derive(Clone, Copy)]
pub(super) struct Drawn {
    pub(super) chart: Memo<Chart>,
    pub(super) folds: RwSignal<HashSet<String>>,
    pub(super) window: RwSignal<(f64, f64)>,
    pub(super) view_top: RwSignal<f64>,
    pub(super) shown_nodes: Memo<Vec<String>>,
    node_at: Memo<HashMap<String, usize>>,
    shown_lines: Memo<Vec<String>>,
    line_at: Memo<HashMap<String, usize>>,
}

fn index(keys: &[String]) -> HashMap<String, usize> {
    keys.iter()
        .enumerate()
        .map(|(i, k)| (k.clone(), i))
        .collect()
}

impl Drawn {
    pub(super) fn new(
        chart: Memo<Chart>,
        folds: RwSignal<HashSet<String>>,
        window: RwSignal<(f64, f64)>,
        view_top: RwSignal<f64>,
    ) -> Self {
        let node_keys = Memo::new(move |_| {
            chart.with(|c| c.nodes.iter().map(|n| n.key.clone()).collect::<Vec<_>>())
        });
        let line_keys = Memo::new(move |_| {
            chart.with(|c| c.lines.iter().map(|l| l.key.clone()).collect::<Vec<_>>())
        });
        let shown_nodes = Memo::new(move |_| {
            let (a, b) = window.get();
            chart.with(|c| {
                c.nodes
                    .iter()
                    .filter(|n| n.y < b && n.stick.unwrap_or(n.y + n.h) > a)
                    .map(|n| n.key.clone())
                    .collect()
            })
        });
        let shown_lines = Memo::new(move |_| {
            let (a, b) = window.get();
            chart.with(|c| {
                c.lines
                    .iter()
                    .filter(|l| l.path.top - SLACK < b && l.path.bottom + SLACK > a)
                    .map(|l| l.key.clone())
                    .collect()
            })
        });
        Drawn {
            chart,
            folds,
            window,
            view_top,
            shown_nodes,
            node_at: Memo::new(move |_| node_keys.with(|k| index(k))),
            shown_lines,
            line_at: Memo::new(move |_| line_keys.with(|k| index(k))),
        }
    }

    fn fold(self, client: &str) {
        self.folds.update(|f| {
            if !f.remove(client) {
                f.insert(client.to_string());
            }
        });
    }

    fn node(self, key: String) -> Memo<Node> {
        Memo::new(move |prev: Option<&Node>| {
            let at = self.node_at.with(|m| m.get(&key).copied());
            at.and_then(|i| self.chart.with(|c| c.nodes.get(i).cloned()))
                .or_else(|| prev.cloned())
                .unwrap_or_default()
        })
    }

    fn line(self, key: String) -> Memo<Line> {
        Memo::new(move |prev: Option<&Line>| {
            let at = self.line_at.with(|m| m.get(&key).copied());
            at.and_then(|i| self.chart.with(|c| c.lines.get(i).cloned()))
                .or_else(|| prev.cloned())
                .unwrap_or_default()
        })
    }

    pub(super) fn tip(self, key: &str) -> Option<String> {
        let at = self.node_at.with(|m| m.get(key).copied())?;
        self.chart
            .with(|c| c.nodes.get(at).map(|n| n.tip.clone()))
            .filter(|t| !t.is_empty())
    }
}

impl Node {
    fn left(&self) -> f64 {
        COLS[self.col].0
    }

    fn wide(&self) -> f64 {
        COLS[self.col].1
    }

    fn dot(&self) -> bool {
        !self.server && self.col == 3 && !self.dashed && self.health == Health::Healthy
    }

    fn text_x(&self) -> f64 {
        self.left() + if self.dot() { 21.0 } else { 10.0 }
    }

    fn end_x(&self) -> f64 {
        self.left() + self.wide() - 8.0
    }

    fn shown_title(&self) -> String {
        let badge_px = self
            .badge
            .as_ref()
            .map_or(0.0, |b| b.len() as f64 * 6.5 + 8.0);
        let room = ((self.wide() - (self.text_x() - self.left()) - 8.0 - badge_px) / 7.0).max(4.0);
        fit(&self.title, room as usize)
    }

    fn sub_rows(&self) -> usize {
        let room = (((self.h - 24.0) / 14.0).floor() as usize).max(1);
        self.sub.lines().count().min(room)
    }

    fn sub_line(&self, i: usize) -> String {
        let room = ((self.wide() - 18.0) / 5.4).max(4.0) as usize;
        fit(self.sub.lines().nth(i).unwrap_or_default(), room)
    }

    fn box_class(&self) -> String {
        let fill = if self.server {
            "fill-accent-weak stroke-accent"
        } else {
            self.health.node()
        };
        format!("{fill} group-hover:stroke-accent group-hover:stroke-2")
    }

    fn badge_class(&self) -> &'static str {
        match self.health {
            Health::Down => "text-[10.5px] font-semibold fill-danger",
            Health::Degraded => "text-[10.5px] font-semibold fill-warning",
            _ => "text-[10.5px] font-semibold fill-text-muted",
        }
    }
}

fn act(s: Store, dr: Drawn, n: Memo<Node>) {
    match n.with_untracked(|n| n.act.clone()) {
        Some(Act::Open(r)) => {
            s.flow_open.set(false);
            route::go(r);
        }
        Some(Act::Fold { client, .. }) => dr.fold(&client),
        None => {}
    }
}

fn folded(n: &Node) -> Option<&'static str> {
    match n.act {
        Some(Act::Fold { open, .. }) => Some(if open { "true" } else { "false" }),
        _ => None,
    }
}

fn hover_at(e: &web_sys::MouseEvent, key: &str, hover: Hover, wrap: NodeRef<html::Div>) {
    let Some(el) = wrap.get_untracked() else {
        return;
    };
    let r = el.get_bounding_client_rect();
    let top = e
        .current_target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .map_or(e.client_y() as f64, |n| n.get_bounding_client_rect().top());
    let x = e.client_x() as f64 - r.left() + el.scroll_left() as f64 + 12.0;
    let y = top - r.top() + el.scroll_top() as f64 - 30.0;
    hover.set(Some((key.to_string(), x, y)));
}

fn node_view(
    key: String,
    dr: Drawn,
    only_problems: Signal<bool>,
    hover: Hover,
    wrap: NodeRef<html::Div>,
) -> impl IntoView {
    let s = store();
    let n = dr.node(key.clone());
    let at = move |f: fn(&Node) -> f64| move || n.with(f);
    let top = Memo::new(move |_| {
        n.with(|n| match n.stick {
            Some(bottom) => sticky_y(n.y, bottom, n.h, dr.view_top.get()),
            None => n.y,
        })
    });
    let below = move |dy: f64| move || top.get() + dy;
    let opacity = move || {
        if only_problems.get() && !n.with(|n| n.problem) {
            "0.28"
        } else {
            "1"
        }
    };
    let hover_key = key.clone();
    let sub = move |i: usize| {
        view! {
            <text x=at(|n| n.left() + 10.0) y=move || top.get() + 31.0 + i as f64 * 14.0 class="fill-text-muted text-[11px]">{move || n.with(|n| n.sub_line(i))}</text>
        }
    };
    view! {
        <g
            class=move || n.with(|n| if n.act.is_some() { "group cursor-pointer outline-none [&:focus-visible>rect]:stroke-accent [&:focus-visible>rect]:stroke-[2.5]" } else { "" })
            opacity=opacity
            data-node=key
            tabindex=move || n.with(|n| folded(n).map(|_| "0"))
            role=move || n.with(|n| folded(n).map(|_| "button"))
            aria-expanded=move || n.with(folded)
            aria-label=move || n.with(|n| folded(n).map(|_| n.tip.clone()))
            on:click=move |_| act(s, dr, n)
            on:keydown=move |e: web_sys::KeyboardEvent| {
                if n.with_untracked(|n| folded(n).is_some()) && (e.key() == "Enter" || e.key() == " ") {
                    e.prevent_default();
                    act(s, dr, n);
                }
            }
            on:mousemove=move |e| {
                if n.with_untracked(|n| !n.tip.is_empty()) {
                    hover_at(&e, &hover_key, hover, wrap);
                }
            }
            on:mouseleave=move |_| hover.set(None)
        >
            <rect
                x=at(Node::left)
                y=below(0.0)
                width=at(Node::wide)
                height=at(|n| n.h)
                rx=move || n.with(|n| if n.server { "12" } else { "9" })
                class=move || n.with(Node::box_class)
                stroke-width=move || n.with(|n| if n.server { "1.5" } else { "1" })
                stroke-dasharray=move || n.with(|n| n.dashed.then_some("4 3"))
            ></rect>
            <circle
                cx=at(|n| n.left() + 12.0)
                cy=below(15.0)
                r="3.5"
                class="fill-success"
                display=move || n.with(|n| (!n.dot()).then_some("none"))
            ></circle>
            <text
                x=at(Node::text_x)
                y=below(17.0)
                class=move || n.with(|n| if n.col == 4 { "fill-text font-mono text-[11.5px] font-semibold" } else { "fill-text text-[12px] font-semibold" })
            >{move || n.with(Node::shown_title)}</text>
            <text x=at(Node::end_x) y=below(17.0) text-anchor="end" class="fill-text-muted font-mono text-[11px]">{move || n.with(|n| n.right.clone())}</text>
            <For each=move || 0..n.with(Node::sub_rows) key=|i| *i children=sub/>
            <text x=at(Node::end_x) y=below(17.0) text-anchor="end" class=move || n.with(Node::badge_class)>{move || n.with(|n| n.badge.clone().unwrap_or_default())}</text>
        </g>
    }
}

fn line_view(key: String, dr: Drawn, only_problems: Signal<bool>) -> impl IntoView {
    let l = dr.line(key.clone());
    view! {
        <path
            d=move || l.with(|l| match &l.stick {
                Some(s) => s.d(dr.view_top.get()),
                None => l.path.d.clone(),
            })
            fill="none"
            class=move || l.with(|l| l.stroke)
            stroke-width=move || l.with(|l| format!("{:.2}", l.width))
            stroke-linecap="round"
            stroke-dasharray=move || l.with(|l| l.dashed.then_some("4 4"))
            opacity=move || if only_problems.get() && !l.with(|l| l.problem) { "0.07" } else { "1" }
            data-line=key
        ></path>
    }
}

pub(super) fn flow_svg(
    dr: Drawn,
    only_problems: Signal<bool>,
    hover: Hover,
    wrap: NodeRef<html::Div>,
) -> impl IntoView {
    let labels = LABELS
        .iter()
        .zip(COLS)
        .map(|(l, (x, _))| view! { <text x=x y="12" class="fill-text-faint text-[10.5px] font-semibold tracking-[0.06em] uppercase">{*l}</text> })
        .collect_view();
    view! {
        <svg
            viewBox=move || dr.chart.with(|c| format!("0 0 {WIDTH} {}", c.height))
            width="100%"
            class="block min-w-[1000px]"
            role="group"
            aria-label=move || dr.chart.with(|c| c.alt.clone())
            data-testid="flow-svg"
            data-nodes=move || dr.chart.with(|c| c.nodes.len().to_string())
        >
            {labels}
            <g>
                <For each=move || dr.shown_lines.get() key=String::clone children=move |k| line_view(k, dr, only_problems)/>
            </g>
            <For each=move || dr.shown_nodes.get() key=String::clone children=move |k| node_view(k, dr, only_problems, hover, wrap)/>
        </svg>
    }
}

pub(super) fn flow_links(s: Store, dr: Drawn) -> impl IntoView {
    let links = Memo::new(move |_| {
        dr.chart.with(|c| {
            c.nodes
                .iter()
                .filter_map(|n| {
                    let r = n.act.as_ref()?.route()?;
                    Some((
                        n.key.clone(),
                        n.tip.trim_end_matches(" →").to_string(),
                        r.clone(),
                    ))
                })
                .collect::<Vec<_>>()
        })
    });
    let link = move |(_, label, to): (String, String, crate::route::Route)| {
        view! {
            <li>
                <a href=route::href(&to) on:click=move |e| {
                    e.prevent_default();
                    s.flow_open.set(false);
                    route::go(to.clone());
                }>{label}</a>
            </li>
        }
    };
    view! { <For each=move || links.get() key=|(k, label, _)| (k.clone(), label.clone()) children=link/> }
}
