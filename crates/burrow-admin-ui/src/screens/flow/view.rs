use burrow_ui_kit::{Icon, Modal, Segmented, Tooltip, button_class};
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::route;
use crate::store::{Store, store};

use super::{COLS, Chart, Health, LABELS, Node, WIDTH, fit, layout};

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
    let room_lines = (((n.h - 24.0) / 14.0).floor() as usize).max(1);
    let lines: Vec<String> = n.sub.lines().take(room_lines).map(str::to_string).collect();
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
        view! { <text x=x + w - 8.0 y=y + 17.0 text-anchor="end" class=format!("text-[10.5px] font-semibold {tone}")>{b}</text> }
    });
    let dot = (!n.server && n.col == 3 && !n.dashed && n.health == Health::Healthy)
        .then(|| view! { <circle cx=x + 12.0 cy=y + 15.0 r="3.5" class="fill-success"></circle> });
    let tx = if dot.is_some() { x + 21.0 } else { x + 10.0 };
    let badge_px = n.badge.as_ref().map_or(0.0, |b| b.len() as f64 * 6.5 + 8.0);
    let room = ((w - (tx - x) - 8.0 - badge_px) / 7.0).max(4.0) as usize;
    let sub_room = ((w - 18.0) / 5.4).max(4.0) as usize;
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
                    let top = e
                        .current_target()
                        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                        .map_or(e.client_y() as f64, |n| n.get_bounding_client_rect().top());
                    hover.set(Some((tip.clone(), e.client_x() as f64 - r.left() + el.scroll_left() as f64 + 12.0, top - r.top() + el.scroll_top() as f64 - 30.0)));
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
            <text x=tx y=y + 17.0 class=if n.col == 4 { "fill-text font-mono text-[11.5px] font-semibold" } else { "fill-text text-[12px] font-semibold" }>{fit(&n.title, room)}</text>
            {(!n.right.is_empty()).then(|| view! { <text x=x + w - 8.0 y=y + 17.0 text-anchor="end" class="fill-text-muted font-mono text-[11px]">{n.right.clone()}</text> })}
            {lines.into_iter().enumerate().map(|(i, l)| view! {
                <text x=x + 10.0 y=y + 31.0 + i as f64 * 14.0 class="fill-text-muted text-[11px]">{fit(&l, sub_room)}</text>
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
        layout(&doc, status.as_deref(), &rates, s.me().as_deref())
    });
    let panel: NodeRef<html::Div> = NodeRef::new();
    let wrap: NodeRef<html::Div> = NodeRef::new();
    let hover: RwSignal<Option<(String, f64, f64)>> = RwSignal::new(None);
    restore_focus(s, chart, only_problems, hover);
    let close = move || s.flow_open.set(false);
    let filter = Signal::derive(move || {
        if only_problems.get() {
            "problems".to_string()
        } else {
            "all".to_string()
        }
    });
    let svg = flow_svg(chart, only_problems, hover, wrap);
    let links = flow_links(s, chart);
    view! {
        <Modal
            open=s.flow_open
            on_close=Callback::new(move |_: ()| close())
            label="Flow".to_string()
            class="fixed top-14 right-24 left-24 z-50 flex max-h-[calc(100vh-80px)] flex-col rounded-[16px] border border-border-strong bg-surface shadow-overlay max-lg:right-4 max-lg:left-4 max-sm:inset-0 max-sm:max-h-none max-sm:rounded-none dark:bg-surface-2"
            testid="flow"
            panel=panel
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
                        <Tooltip x=x y=y text=t/>
                    })}
                    <ul class="sr-only" aria-label="Open from the flow chart">{links}</ul>
                </div>
                {legend()}
        </Modal>
    }
}

fn restore_focus(
    s: Store,
    chart: Memo<Chart>,
    only_problems: RwSignal<bool>,
    hover: RwSignal<Option<(String, f64, f64)>>,
) {
    Effect::new(move |was: Option<bool>| {
        let open = s.flow_open.get();
        if open && was != Some(true) {
            let c = chart.get_untracked();
            only_problems.set(c.services > 12 && c.problems > 0);
        } else if !open && was == Some(true) {
            hover.set(None);
        }
        open
    });
}

fn flow_svg(
    chart: Memo<Chart>,
    only_problems: RwSignal<bool>,
    hover: RwSignal<Option<(String, f64, f64)>>,
    wrap: NodeRef<html::Div>,
) -> impl IntoView + Copy {
    move || {
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
    }
}

fn flow_links(s: Store, chart: Memo<Chart>) -> impl IntoView + Copy {
    move || {
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
    }
}

fn legend() -> impl IntoView {
    view! {
    <div class="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border px-4 py-2.5 text-xs text-text-muted">
        <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-success"></span>"healthy"</span>
        <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-warning"></span>"degraded"</span>
        <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-danger"></span>"down"</span>
        <span class="flex items-center gap-1.5"><span class="w-4 border-t border-dashed border-text-faint"></span>"offline"</span>
        <span class="flex items-center gap-1.5"><span class="h-0.5 w-4 rounded bg-accent"></span>"server ↔ client channels"</span>
        <span>"line weight = open connections; client → target lines are fixed"</span>
        <span class="ml-auto">"Click a node to open that service or client · Esc closes"</span>
    </div>
    }
}
