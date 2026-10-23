use burrow_admin_proto::schema::Entity;
use burrow_config::{ClientEntry, Entry, Protocol, ServiceEntry, VisitorEntry, entity_of_section};
use std::collections::VecDeque;

use burrow_admin_logic::rates::{axis_max, human_bits, mbit};
use burrow_admin_proto::diag::Severity;
use burrow_admin_proto::msg::State;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_ui_kit::{
    ChartData, EmptyState, Icon, Kbd, Kpi, LineChart, PillTabs, Segmented, Select, StatusDot,
    Table, Tone, button_class, tag_class,
};
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::form::plain;
use crate::format;
use crate::route::{self, Link, Route};
use crate::screens::paths::clients_of;
use crate::store::store;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub tone: Tone,
    pub title: String,
    pub detail: String,
    pub to: Option<Route>,
}

fn rank(t: Tone) -> u8 {
    match t {
        Tone::Danger => 0,
        Tone::Warning => 1,
        _ => 2,
    }
}

fn names(v: &Value, section: &str) -> Vec<String> {
    v.get(section)
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn link_of(path: &[String]) -> Option<Route> {
    match path {
        [s, n, ..] if crate::route::SECTIONS.contains(&s.as_str()) => {
            Some(Route::Detail(s.clone(), n.clone()))
        }
        [_, ..] => Some(Route::Settings),
        [] => None,
    }
}

pub fn attention(values: &Value, state: Option<&State>, status: Option<&Status>) -> Vec<Item> {
    let mut out = Vec::new();
    if let Some(st) = state {
        if !st.running.ok {
            out.push(Item {
                key: "running".into(),
                tone: Tone::Danger,
                title: "The config file has errors".into(),
                detail: "The server keeps running the last good version until the file is fixed."
                    .into(),
                to: Some(Route::About),
            });
        }
        for p in &st.runtime.restart_pending {
            out.push(Item {
                key: format!("restart:{p}"),
                tone: Tone::Info,
                title: format!("Restart needed for {p}"),
                detail:
                    "Applied to the file; the running server keeps the old value until it restarts."
                        .into(),
                to: Some(Route::Settings),
            });
        }
        for d in st.diags.iter().filter(|d| d.severity == Severity::Warning) {
            out.push(Item {
                key: format!(
                    "diag:{}:{}",
                    d.code,
                    d.path.as_ref().map(|p| p.join(".")).unwrap_or_default()
                ),
                tone: Tone::Warning,
                title: d.message.clone(),
                detail: d
                    .help
                    .clone()
                    .map(|h| format!("{} · {h}", d.code))
                    .unwrap_or_else(|| d.code.clone()),
                to: d.path.as_deref().and_then(link_of),
            });
        }
    }
    if let Some(status) = status {
        let online = |c: &str| {
            status
                .clients
                .iter()
                .find(|x| x.name == c)
                .is_some_and(|x| x.connected)
        };
        let mut by_client: std::collections::BTreeMap<String, Hits> =
            std::collections::BTreeMap::new();
        let mut own = Vec::new();
        if let Some(services) = ServiceEntry::entries(values) {
            for (name, svc) in services {
                let cs = clients_of(svc);
                let up: Vec<&String> = cs.iter().filter(|c| online(c)).collect();
                let off: Vec<&String> = cs.iter().filter(|c| !online(c)).collect();
                let st = status.services.iter().find(|x| &x.name == name);
                let to = Some(Route::Detail(ServiceEntry::section().into(), name.clone()));
                if up.is_empty() {
                    if let [c] = off.as_slice() {
                        by_client
                            .entry((*c).clone())
                            .or_default()
                            .0
                            .push(name.clone());
                        continue;
                    }
                    own.push(Item {
                        key: format!("down:{name}"),
                        tone: Tone::Danger,
                        title: format!("Service {name} is down"),
                        detail: if cs.is_empty() {
                            "It has no client.".to_string()
                        } else {
                            format!(
                                "None of its {} clients is online: {}.",
                                cs.len(),
                                cs.join(", ")
                            )
                        },
                        to,
                    });
                    continue;
                }
                let failing: Vec<&str> = st
                    .map(|x| {
                        x.health
                            .iter()
                            .filter(|h| !h.up)
                            .map(|h| h.client.as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                if !failing.is_empty() {
                    own.push(Item {
                        key: format!("health:{name}"),
                        tone: Tone::Warning,
                        title: format!("Service {name}: health check failing"),
                        detail: format!("Failing on {}.", failing.join(", ")),
                        to,
                    });
                } else if let [c] = off.as_slice() {
                    by_client.entry((*c).clone()).or_default().1.push((
                        name.clone(),
                        up.len(),
                        cs.len(),
                    ));
                } else if !off.is_empty() {
                    own.push(Item {
                        key: format!("partial:{name}"),
                        tone: Tone::Warning,
                        title: format!(
                            "Service {name} runs on {} of {} clients",
                            up.len(),
                            cs.len()
                        ),
                        detail: format!(
                            "Offline: {}.",
                            off.iter()
                                .map(|c| c.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        to,
                    });
                }
            }
        }
        for (c, (down, partial)) in by_client {
            let mut parts = Vec::new();
            if !down.is_empty() {
                parts.push(format!(
                    "{} {} down",
                    and_list(&down),
                    if down.len() == 1 { "is" } else { "are" }
                ));
            }
            let mut shapes: Vec<(usize, usize)> =
                partial.iter().map(|(_, u, n)| (*u, *n)).collect();
            shapes.sort();
            shapes.dedup();
            for (u, n) in shapes {
                let names: Vec<String> = partial
                    .iter()
                    .filter(|(_, a, b)| (*a, *b) == (u, n))
                    .map(|(x, _, _)| x.clone())
                    .collect();
                parts.push(format!(
                    "{} {} on {u} of {n} clients",
                    and_list(&names),
                    if names.len() == 1 { "runs" } else { "run" }
                ));
            }
            let when = format::left_of(
                &c,
                status
                    .clients
                    .iter()
                    .find(|x| x.name == c)
                    .and_then(|x| x.left_secs_ago),
                status.uptime_secs,
            );
            out.push(Item {
                key: format!("client:{c}"),
                tone: if down.is_empty() {
                    Tone::Warning
                } else {
                    Tone::Danger
                },
                title: format!("Client {c} is offline"),
                detail: format!("{} · {when}", parts.join("; ")),
                to: Some(Route::Detail(ClientEntry::section().into(), c)),
            });
        }
        out.extend(own);
    }
    out.sort_by_key(|i| rank(i.tone));
    out
}

type Hits = (Vec<String>, Vec<(String, usize, usize)>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    Down,
    Partial,
    Health,
}

pub fn service_problems(values: &Value, status: Option<&Status>) -> Vec<(String, Problem)> {
    let Some(status) = status else {
        return Vec::new();
    };
    let online = |c: &str| {
        status
            .clients
            .iter()
            .find(|x| x.name == c)
            .is_some_and(|x| x.connected)
    };
    let mut out: Vec<(String, Problem)> = ServiceEntry::entries(values)
        .map(|m| {
            m.iter()
                .filter_map(|(name, svc)| {
                    let cs = clients_of(svc);
                    let up = cs.iter().filter(|c| online(c)).count();
                    let failing = status
                        .services
                        .iter()
                        .find(|x| &x.name == name)
                        .is_some_and(|x| x.health.iter().any(|h| !h.up));
                    let p = if up == 0 {
                        Problem::Down
                    } else if failing {
                        Problem::Health
                    } else if up < cs.len() {
                        Problem::Partial
                    } else {
                        return None;
                    };
                    Some((name.clone(), p))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn healthy_detail(p: &[(String, Problem)]) -> String {
    match p {
        [] => "all up".into(),
        [(n, Problem::Down)] => format!("{n} is down"),
        [(n, Problem::Health)] => format!("{n} fails its health check"),
        [(n, Problem::Partial)] => format!("{n} runs on fewer clients"),
        many => format!("{} with problems", many.len()),
    }
}

fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

pub fn away(values: &Value, status: Option<&Status>) -> Vec<Item> {
    let Some(status) = status else {
        return Vec::new();
    };
    names(values, ClientEntry::section())
        .into_iter()
        .filter_map(|c| {
            let st = status.clients.iter().find(|x| x.name == c);
            if st.is_some_and(|x| x.connected) {
                return None;
            }
            let serves: Vec<String> = ServiceEntry::entries(values)
                .map(|m| {
                    m.iter()
                        .filter(|(_, v)| clients_of(v).contains(&c))
                        .map(|(n, _)| n.clone())
                        .collect()
                })
                .unwrap_or_default();
            let when = format::left_of(&c, st.and_then(|x| x.left_secs_ago), status.uptime_secs);
            Some(Item {
                key: format!("away:{c}"),
                tone: Tone::Neutral,
                title: c.clone(),
                detail: format!(
                    "{} · {when}",
                    if serves.is_empty() {
                        "no services".to_string()
                    } else {
                        format!("serves {}", serves.join(", "))
                    }
                ),
                to: Some(Route::Detail(ClientEntry::section().into(), c)),
            })
        })
        .collect()
}

pub fn x_ticks(window: usize) -> Vec<String> {
    match window {
        60 => vec![
            "−60 s".into(),
            "−45 s".into(),
            "−30 s".into(),
            "−15 s".into(),
            "now".into(),
        ],
        300 => vec![
            "−5 min".into(),
            "−4 min".into(),
            "−3 min".into(),
            "−2 min".into(),
            "−1 min".into(),
            "now".into(),
        ],
        _ => vec![
            "−15 min".into(),
            "−10 min".into(),
            "−5 min".into(),
            "now".into(),
        ],
    }
}

fn tick(v: f64) -> String {
    if v >= 10.0 || v.fract() == 0.0 {
        format!("{v:.0}")
    } else if v >= 1.0 {
        format!("{v:.1}")
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').to_string()
    }
}

pub fn chart(a: &[Option<f64>], b: &[Option<f64>], window: usize) -> ChartData {
    let take = |s: &[Option<f64>]| -> Vec<Option<f64>> {
        s[s.len().saturating_sub(window)..]
            .iter()
            .map(|v| v.map(mbit))
            .collect()
    };
    let (a, b) = (take(a), take(b));
    let top = a
        .iter()
        .chain(b.iter())
        .flatten()
        .copied()
        .fold(0.0_f64, f64::max);
    let max = if top > 0.0 { axis_max(top) } else { 20.0 };
    let samples = a.iter().filter(|v| v.is_some()).count();
    let empty_note = (a.len() < 30 && a.len() * 2 < window).then(|| {
        format!(
            "Recording since you opened this page · {} so far",
            format::span(a.len() as u64)
        )
    });
    ChartData {
        a: if samples < 2 { Vec::new() } else { a },
        b: if samples < 2 { Vec::new() } else { b },
        slots: window,
        max,
        y_ticks: (0..5).map(|i| tick(max * i as f64 / 4.0)).collect(),
        x_ticks: x_ticks(window),
        empty_note,
    }
}

#[component]
fn Traffic() -> impl IntoView {
    let s = store();
    let service = RwSignal::new(String::new());
    let window = RwSignal::new(900usize);
    let series = Memo::new(move |_| {
        let svc = service.get();
        s.recorder.with(|r| {
            r.series((!svc.is_empty()).then_some(svc.as_str()))
                .map(|(a, b)| {
                    (
                        a.points().collect::<Vec<_>>(),
                        b.points().collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default()
        })
    });
    let data = Signal::derive(move || series.with(|(a, b)| chart(a, b, window.get())));
    let options = Signal::derive(move || {
        let mut o = vec![(String::new(), "All services".to_string())];
        s.status.with(|st| {
            if let Some(st) = st {
                o.extend(st.services.iter().map(|x| (x.name.clone(), x.name.clone())));
            }
        });
        o
    });
    let now = move |out: bool| {
        let svc = service.get();
        let r = s.rates.get();
        let rate = if svc.is_empty() {
            r.total
        } else {
            r.service(&svc)
        };
        human_bits(if out { rate.bytes_out } else { rate.bytes_in })
    };
    let label = Signal::derive(move || {
        let (i, iu) = now(false);
        let (o, ou) = now(true);
        format!(
            "Traffic over the last {} minutes; now {i} {iu} in and {o} {ou} out",
            window.get() / 60
        )
    });
    view! {
        <section class="flex flex-col rounded-card border border-border bg-surface min-[1200px]:col-span-8" data-testid="traffic">
            <div class="flex flex-wrap items-center gap-3 px-4 pt-3.5">
                <h2 class="text-[13.5px] font-bold">"Traffic"</h2>
                <span class="text-xs text-text-muted">"live · recorded while this page is open"</span>
                <span class="ml-auto"></span>
                <div class="w-36">
                    <Select id="traffic-service" small=true label="Service" options=options value=service on_change=Callback::new(move |v: String| service.set(v))/>
                </div>
                <Segmented
                    name="traffic-window"
                    label="Window"
                    options=vec![("60".to_string(), "1m".to_string(), false), ("300".to_string(), "5m".to_string(), false), ("900".to_string(), "15m".to_string(), false)]
                    value=Signal::derive(move || window.get().to_string())
                    on_change=Callback::new(move |v: String| window.set(v.parse().unwrap_or(900)))
                />
            </div>
            <div class="flex flex-wrap items-end gap-6 px-4 pt-3">
                <div data-testid="traffic-in">
                    <div class="flex items-center gap-1.5 text-xs text-text-muted"><span class="h-0.5 w-3.5 rounded bg-accent"></span>"In"</div>
                    <div class="text-2xl font-bold tabular-nums">{move || now(false).0}<small class="ml-1 text-[13px] font-medium text-text-muted">{move || now(false).1}</small></div>
                </div>
                <div data-testid="traffic-out">
                    <div class="flex items-center gap-1.5 text-xs text-text-muted"><span class="h-0.5 w-3.5 rounded bg-udp"></span>"Out"</div>
                    <div class="text-2xl font-bold tabular-nums">{move || now(true).0}<small class="ml-1 text-[13px] font-medium text-text-muted">{move || now(true).1}</small></div>
                </div>
                <div class="ml-auto text-right text-[11.5px] text-text-muted" data-testid="traffic-note">
                    <div>{move || format!("{} · Mbit/s", if service.get().is_empty() { "all services".to_string() } else { service.get() })}</div>
                </div>
            </div>
            <div class="flex h-[240px] px-4 pt-3">
                <LineChart data=data label=label testid="traffic-chart"/>
            </div>
            <p class="mt-auto flex items-center gap-1.5 border-t border-border px-4 py-2.5 text-xs text-text-muted">
                <Icon name="info" small=true/>
                "Recorded by your browser from the live status (1 s), kept in memory for 15 min. The server keeps no history; reloading the page starts over."
            </p>
        </section>
    }
}

fn row_view(i: Item, icon: bool) -> impl IntoView {
    let tone = i.tone;
    let glyph = match tone {
        Tone::Danger => "alert-circle",
        Tone::Warning => "alert",
        Tone::Info => "info",
        _ => "monitor",
    };
    let inner = view! {
        <span class=format!("mt-0.5 shrink-0 {}", tone.text())>
            {if icon { view! { <Icon name=glyph/> }.into_any() } else { view! { <span class="mt-1 block size-2 rounded-full bg-text-faint" aria-hidden="true"></span> }.into_any() }}
        </span>
        <span class="min-w-0">
            <span class="block text-sm font-semibold">{i.title.clone()}</span>
            <span class="block text-[12.5px] text-text-muted">{i.detail.clone()}</span>
        </span>
    };
    let class = "flex gap-2.5 px-4 py-[11px] text-text no-underline hover:bg-surface-2 dark:hover:bg-surface-3/50";
    let li = "border-b border-rowline last:border-b-0";
    view! {
        <li class=li data-item=i.key.clone() data-tone=tone.name()>
            {match i.to {
                Some(to) => view! { <Link to=to class=class>{inner}</Link> }.into_any(),
                None => view! { <div class=class>{inner}</div> }.into_any(),
            }}
        </li>
    }
}

fn numbered(items: Vec<Item>) -> Vec<(usize, Item)> {
    items.into_iter().enumerate().collect()
}

fn hidden_below(list: &web_sys::HtmlElement) -> usize {
    let bottom = list.scroll_top() + list.client_height();
    let Ok(items) = list.query_selector_all("li") else {
        return 0;
    };
    (0..items.length())
        .filter_map(|i| {
            items
                .get(i)
                .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
        })
        .filter(|el| el.offset_height() > 0 && el.offset_top() + el.offset_height() > bottom + 2)
        .count()
}

#[component]
fn Section(
    title: &'static str,
    #[prop(into)] items: Signal<Vec<Item>>,
    tone: Signal<Tone>,
    icon: bool,
    testid: &'static str,
    #[prop(into)] grow: Signal<String>,
    empty: (&'static str, Signal<String>, Signal<String>),
) -> impl IntoView {
    let list: NodeRef<html::Ul> = NodeRef::new();
    let hidden = RwSignal::new(0usize);
    let measure = move || {
        if let Some(el) = list.try_get_untracked().flatten() {
            let el: web_sys::HtmlElement = el.into();
            let n = if el.client_height() > 0 && el.scroll_height() > el.client_height() {
                hidden_below(&el)
            } else {
                0
            };
            if hidden.try_get_untracked().is_some_and(|h| h != n) {
                let _ = hidden.try_set(n);
            }
        }
    };
    Effect::new(move |_| {
        items.track();
        request_animation_frame(measure);
    });
    let resize = window_event_listener(leptos::ev::resize, move |_| measure());
    on_cleanup(move || resize.remove());
    let total = move || items.with(Vec::len);
    view! {
        <div class=move || format!("flex min-h-0 flex-col {}", grow.get()) data-testid=testid>
            <div class="flex shrink-0 items-center gap-2 border-b border-border px-4 py-3">
                <h2 class="text-[13.5px] font-bold">{title}</h2>
                <span class=move || tag_class(tone.get()) data-testid="count">{total}</span>
                <span class="ml-auto"></span>
            </div>
            <div class="relative min-h-0 flex-1">
                {move || if total() == 0 {
                    view! {
                        <div class="flex h-full items-center justify-center py-6">
                            <div class="flex flex-col items-center gap-1.5 px-4 text-center">
                                <span class="text-success"><Icon name=empty.0 large=true/></span>
                                <span class="text-sm font-semibold">{move || empty.1.get()}</span>
                                <span class="text-[12.5px] text-text-muted">{move || empty.2.get()}</span>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    ().into_any()
                }}
                <ul
                    node_ref=list
                    class="h-full overflow-y-auto overscroll-contain [scrollbar-width:thin] max-[1199px]:max-h-[340px]"
                    on:scroll=move |_| measure()
                >
                    <For
                        each=move || numbered(items.get())
                        key=|(i, x)| format!("{i}|{}|{}|{}", x.key, x.title, x.detail)
                        children=move |(_, x)| row_view(x, icon)
                    />
                </ul>
                <div data-testid="fade" class=move || format!("pointer-events-none absolute inset-x-0 bottom-0 h-6 bg-gradient-to-t from-surface to-transparent{}", if hidden.get() > 0 { "" } else { " hidden" })></div>
            </div>
        </div>
    }
}

#[component]
fn Attention() -> impl IntoView {
    let s = store();
    let items = Memo::new(move |_| {
        let v = s.running();
        let st = s.state.get();
        let status = s.status.get();
        attention(&v, st.as_deref(), status.as_deref())
    });
    let away_items = Memo::new(move |_| {
        let v = s.running();
        away(&v, s.status.get().as_deref())
    });
    let seen: StoredValue<Vec<String>> = StoredValue::new(Vec::new());
    let announce = RwSignal::new(String::new());
    Effect::new(move |_| {
        let now: Vec<Item> = items
            .get()
            .into_iter()
            .filter(|i| i.tone == Tone::Danger)
            .collect();
        let before = seen.get_value();
        let fresh: Vec<String> = now
            .iter()
            .filter(|i| !before.contains(&i.key))
            .map(|i| i.title.clone())
            .collect();
        if !fresh.is_empty() && !before.is_empty() {
            announce.set(fresh.join(". "));
        }
        seen.set_value(
            now.into_iter()
                .map(|i| i.key)
                .chain(std::iter::once(String::new()))
                .collect(),
        );
    });
    let longest = move || {
        s.status.with(|st| {
            st.as_ref().and_then(|st| {
                st.clients
                    .iter()
                    .filter(|c| c.connected && !c.local)
                    .max_by_key(|c| c.connected_secs.unwrap_or(0))
                    .map(|c| {
                        format!(
                            "Longest connected: {}, {}.",
                            c.name,
                            format::span(c.connected_secs.unwrap_or(0))
                        )
                    })
            })
        })
    };
    let n_clients = move || names(&s.running(), ClientEntry::section()).len();
    let away_title: &'static str = "Clients away";
    let away_grow = Signal::derive(move || {
        if away_items.with(Vec::len) >= 2 {
            "shrink-[2] grow basis-auto min-[1200px]:min-h-[156px]".to_string()
        } else {
            "shrink-0 grow basis-auto".to_string()
        }
    });
    let away_tone = Signal::derive(move || {
        if away_items.with(Vec::is_empty) {
            Tone::Success
        } else {
            Tone::Warning
        }
    });
    let away_title_empty = Signal::derive(move || {
        let n = n_clients();
        if n == 0 {
            "No clients yet".to_string()
        } else {
            format!("All {n} clients online")
        }
    });
    let away_msg = Signal::derive(move || longest().unwrap_or_default());
    view! {
        <section class="relative rounded-card border border-border bg-surface min-[1200px]:col-span-4" data-testid="attention-card">
            <div class="flex flex-col overflow-hidden rounded-card min-[1200px]:absolute min-[1200px]:inset-0 [&>[data-testid=away]]:border-t [&>[data-testid=away]]:border-border">
                <Section
                    title="Needs attention"
                    items=items
                    tone=Signal::derive(move || if items.with(Vec::is_empty) { Tone::Success } else { Tone::Warning })
                    icon=true
                    testid="attention"
                    grow="grow shrink basis-auto".to_string()
                    empty=("check", Signal::stored("Nothing needs attention".to_string()), Signal::stored("Every service has a client online and every check passes.".to_string()))
                />
                <Section
                    title=away_title
                    items=away_items
                    tone=away_tone
                    icon=false
                    testid="away"
                    grow=away_grow
                    empty=("monitor", away_title_empty, away_msg)
                />
            </div>
            <p class="sr-only" aria-live="polite">{move || announce.get()}</p>
        </section>
    }
}

fn mobile_cards(section: &'static str) -> impl IntoView {
    let rows = crate::screens::entity::rows_for(section);
    view! {
        <div class="pt-2 pb-1 sm:hidden" data-testid=format!("mobile-{section}")>
            {rows.into_iter().map(|r| view! { <crate::screens::ListCard row=r selected=false/> }).collect_view()}
        </div>
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Refused(f64, u64);

#[component]
pub fn Overview() -> impl IntoView {
    let s = store();
    let refused_log: RwSignal<VecDeque<Refused>> = RwSignal::new(VecDeque::new());
    Effect::new(move |_| {
        let at = s.status_at.get();
        let n = s
            .status
            .with(|st| st.as_ref().map(|st| st.handshakes_refused));
        if let Some(n) = n {
            refused_log.update(|l| {
                l.push_back(Refused(at, n));
                while l.front().is_some_and(|r| at - r.0 > 60_000.0) {
                    l.pop_front();
                }
            });
        }
    });
    let values = Memo::new(move |_| s.running());
    let online = move |name: &str| {
        s.status.with(|st| {
            st.as_ref()
                .and_then(|st| st.clients.iter().find(|c| c.name == name))
                .is_some_and(|c| c.connected)
        })
    };
    let clients_now = Signal::derive(move || {
        let n = names(&values.get(), ClientEntry::section());
        let up = n.iter().filter(|c| online(c)).count();
        (up, n.len())
    });
    let counts = Signal::derive(move || {
        let v = values.get();
        let (up, n) = clients_now.get();
        let vis = names(&v, VisitorEntry::section()).len();
        let active = s.status.with(|st| {
            st.as_ref()
                .map(|st| st.visitors.iter().filter(|x| s.in_use(x)).count())
                .unwrap_or(0)
        });
        vec![
            names(&v, ServiceEntry::section()).len().to_string(),
            format!("{up}/{n}"),
            format!("{active}/{vis}"),
        ]
    });
    let down = Signal::derive(move || {
        let v = values.get();
        let st = s.status.get();
        (
            names(&v, ServiceEntry::section()).len(),
            service_problems(&v, st.as_deref()),
        )
    });
    let sub = move || {
        let st = s.status.get();
        let w = s.welcome.get();
        let mut parts: Vec<String> = Vec::new();
        if let Some(w) = &w {
            parts.push(format!("v{}", w.server.version));
        }
        if let Some(st) = &st {
            parts.push(format!("up {}", format::span(st.uptime_secs)));
        }
        let rev = s.short_rev();
        if !rev.is_empty() {
            parts.push(format!("rev {rev}"));
        }
        parts.join(" · ")
    };
    let tab = RwSignal::new(
        match route::query("tab").as_deref().and_then(entity_of_section) {
            Some(Entity::Client) => ClientEntry::section(),
            Some(Entity::Visitor) => VisitorEntry::section(),
            _ => ServiceEntry::section(),
        },
    );
    let rate = move |name: &str| {
        let (v, u) = human_bits(s.rates.with(|r| r.service(name).bytes_in));
        format!("{v} {u}")
    };
    let services_table = move || {
        let v = values.get();
        let st = s.status.get();
        let m = ServiceEntry::entries(&v).cloned().unwrap_or_default();
        if m.is_empty() {
            return view! { <EmptyState title="No services yet" message="Services publish a port of a client on this server." testid="services-empty"/> }.into_any();
        }
        let rows = m
            .iter()
            .map(|(name, svc)| {
                let x = st.as_ref().and_then(|st| st.services.iter().find(|y| &y.name == name)).cloned();
                let protocol = ServiceEntry::PROTOCOL.get(svc).map(plain).unwrap_or_else(|| Protocol::default().wire().into());
                let cs = clients_of(svc);
                let up = cs.iter().filter(|c| online(c)).count();
                let health = crate::screens::entity::health_tag(st.is_some().then_some(up), cs.len(), x.as_ref().map(|x| x.health.as_slice()).unwrap_or(&[]));
                let label = name.clone();
                view! {
                    <tr data-row=name.clone()>
                        <th scope="row"><Link to=Route::Detail(ServiceEntry::section().into(), name.clone()) class="text-text no-underline hover:underline">{label}</Link></th>
                        <td><span class=tag_class(if protocol == Protocol::Udp.wire() { Tone::Violet } else { Tone::Accent })>{protocol.to_uppercase()}</span></td>
                        <td>{match ServiceEntry::BIND.get(svc).filter(|b| !b.is_null()) {
                            Some(b) => view! { <span class="font-mono text-[13px]">{plain(b)}</span> }.into_any(),
                            None => view! { <span class="text-text-faint">"private"</span> }.into_any(),
                        }}</td>
                        <td class="font-mono text-[13px] text-text-muted">{ServiceEntry::TARGET.get(svc).map(plain).unwrap_or_default()}</td>
                        <td class="text-right tabular-nums" data-cell="open">{x.as_ref().map(|x| format::count(x.open)).unwrap_or_default()}</td>
                        <td class="text-right tabular-nums" data-cell="rate">{rate(name)}</td>
                        <td>{health}</td>
                    </tr>
                }
            })
            .collect_view();
        view! { <div class="max-sm:hidden"><Table head=vec!["Service", "_", "Public", "Target", ">Open", ">In", "_Health"] testid="overview-services">{rows}</Table></div> {mobile_cards(ServiceEntry::section())} }.into_any()
    };
    let clients_table = move || {
        let v = values.get();
        let n = names(&v, ClientEntry::section());
        if n.is_empty() {
            return view! { <EmptyState title="No clients yet" message="A client is a machine that connects out to this server." testid="clients-empty"/> }.into_any();
        }
        let st = s.status.get();
        let rows = n
            .into_iter()
            .map(|name| {
                let c = st.as_ref().and_then(|st| st.clients.iter().find(|c| c.name == name)).cloned();
                let on = c.as_ref().is_some_and(|c| c.connected);
                let since = c
                    .as_ref()
                    .and_then(|c| match (c.connected_secs, c.left_secs_ago) {
                        (Some(x), _) => Some(format!("for {}", format::span(x))),
                        (None, Some(a)) => Some(format!("left {}", format::ago(a))),
                        _ => None,
                    })
                    .unwrap_or_else(|| format::left_of(&name, None, st.as_ref().map(|s| s.uptime_secs).unwrap_or(0)));
                let serves: Vec<String> = ServiceEntry::entries(&v)
                    .map(|m| m.iter().filter(|(_, x)| clients_of(x).contains(&name)).map(|(k, _)| k.clone()).collect())
                    .unwrap_or_default();
                let label = name.clone();
                view! {
                    <tr data-row=name.clone() data-online=on.to_string()>
                        <th scope="row"><Link to=Route::Detail(ClientEntry::section().into(), name.clone()) class="text-text no-underline hover:underline"><StatusDot on=on text=label/></Link></th>
                        <td class="font-mono text-[13px]">{c.as_ref().and_then(|c| c.from.clone()).unwrap_or_default()}</td>
                        <td class="text-right tabular-nums">{c.as_ref().map(|c| c.open).unwrap_or(0)}</td>
                        <td class="text-right tabular-nums">{c.as_ref().map(|c| c.idle_channels).unwrap_or(0)}</td>
                        <td>{since}</td>
                        <td class="text-text-muted">{serves.join(", ")}</td>
                    </tr>
                }
            })
            .collect_view();
        view! { <div class="max-sm:hidden"><Table head=vec!["Client", "From", ">Open", ">Idle channels", "Since", "Serves"] testid="overview-clients">{rows}</Table></div> {mobile_cards(ClientEntry::section())} }.into_any()
    };
    let visitors_table = move || {
        let v = values.get();
        let n = names(&v, VisitorEntry::section());
        if n.is_empty() {
            return view! { <EmptyState title="No visitors yet" message="A visitor reaches private services with burrow visit." testid="visitors-empty"/> }.into_any();
        }
        let st = s.status.get();
        let rows = n
            .into_iter()
            .map(|name| {
                let x = st.as_ref().and_then(|st| st.visitors.iter().find(|c| c.name == name)).cloned();
                let open = x.as_ref().map(|x| x.open).unwrap_or(0);
                let admin = VisitorEntry::ADMIN.at(&v, &name).map(plain);
                let may: Vec<String> = ServiceEntry::entries(&v)
                    .map(|m| m.iter().filter(|(_, s)| ServiceEntry::VISITORS.get(s).and_then(Value::as_array).is_some_and(|a| a.iter().any(|y| y.as_str() == Some(name.as_str())))).map(|(k, _)| k.clone()).collect())
                    .unwrap_or_default();
                let label = name.clone();
                view! {
                    <tr data-row=name.clone()>
                        <th scope="row">
                            <Link to=Route::Detail(VisitorEntry::section().into(), name.clone()) class="text-text no-underline hover:underline">{label}</Link>
                            {x.as_ref().is_some_and(|x| s.in_use(x)).then(|| view! { <span class=format!("ml-2 {}", tag_class(Tone::Success))>"in use now"</span> })}
                        </th>
                        <td>{admin.map(|a| view! { <span class=tag_class(if a == "write" { Tone::Danger } else { Tone::Accent })>{format!("admin · {a}")}</span> })}</td>
                        <td class="text-text-muted">{may.join(", ")}</td>
                        <td class="text-right tabular-nums">{open}</td>
                        <td class="text-right tabular-nums">{x.as_ref().map(|x| format::count(x.total)).unwrap_or_default()}</td>
                        <td class="text-text-muted" data-last-used="">{x.as_ref().map(|x| if s.in_use(x) { "now".to_string() } else { x.last_seen_secs.map(format::ago).unwrap_or_else(|| "not seen".into()) }).unwrap_or_default()}</td>
                    </tr>
                }
            })
            .collect_view();
        view! { <div class="max-sm:hidden"><Table head=vec!["Visitor", "Admin", "Services it may use", ">Open", ">Total", "Last seen"] testid="overview-visitors">{rows}</Table></div> {mobile_cards(VisitorEntry::section())} }.into_any()
    };
    let kpi_link = "rounded-card no-underline text-text hover:[&>div]:border-border-strong";
    view! {
        <div class="px-8 pt-7 pb-12 max-sm:px-4" data-testid="overview">
            <header class="mb-[22px] flex flex-wrap items-start gap-3">
                <div class="min-w-0 flex-1">
                    <h1 class="text-2xl font-bold tracking-[-0.015em]">"Overview"</h1>
                    <p class="mt-1 text-sm text-text-muted" data-testid="overview-sub">{sub}</p>
                </div>
                <button type="button" class=button_class("secondary", false) data-testid="flow-open" on:click=move |_| s.flow_open.set(true)>
                    <Icon name="network"/>
                    "Flow chart"
                    <Kbd text="F"/>
                </button>
            </header>
            <div class="grid grid-cols-2 gap-4 md:grid-cols-6 xl:grid-cols-12">
                <Link to=Route::List(ClientEntry::section().into()) class=format!("md:col-span-2 {kpi_link}") testid="kpi-clients-link">
                    <Kpi
                        label="Clients online"
                        icon="monitor"
                        value=Signal::derive(move || clients_now.get().0.to_string())
                        unit=Signal::derive(move || format!("/ {}", clients_now.get().1))
                        detail=Signal::derive(move || { let (u, n) = clients_now.get(); if u == n { "all connected".to_string() } else { format!("{} away", n - u) } })
                        testid="kpi-clients"
                    />
                </Link>
                <Link to=Route::List(ServiceEntry::section().into()) class=format!("md:col-span-2 {kpi_link}") testid="kpi-services-link">
                    <Kpi
                        label="Services healthy"
                        icon="stack"
                        value=Signal::derive(move || { let (n, bad) = down.get(); (n - bad.len().min(n)).to_string() })
                        unit=Signal::derive(move || format!("/ {}", down.get().0))
                        detail=Signal::derive(move || healthy_detail(&down.get().1))
                        testid="kpi-services"
                    />
                </Link>
                <div class="md:col-span-2">
                    <Kpi
                        label="Open connections"
                        icon="network"
                        value=Signal::derive(move || s.status.with(|st| format::count(st.as_ref().map(|st| st.services.iter().map(|x| x.open).sum()).unwrap_or(0))))
                        detail=Signal::derive(move || s.status.with(|st| format!("{} since start", format::count(st.as_ref().map(|st| st.services.iter().map(|x| x.total).sum()).unwrap_or(0)))))
                        testid="kpi-open"
                    />
                </div>
                <div class="md:col-span-2">
                    <Kpi
                        label="Traffic now"
                        icon="activity"
                        value=Signal::derive(move || human_bits(s.rates.with(|r| r.total.bytes_in)).0)
                        unit=Signal::derive(move || human_bits(s.rates.with(|r| r.total.bytes_in)).1.to_string())
                        detail=Signal::derive(move || { let (o, u) = human_bits(s.rates.with(|r| r.total.bytes_out)); format!("in · {o} {u} out") })
                        testid="kpi-traffic"
                    />
                </div>
                <div class="md:col-span-2">
                    <Kpi
                        label="Visitors in use"
                        icon="person"
                        value=Signal::derive(move || {
                            let active = s.status.with(|st| st.as_ref().map(|st| st.visitors.iter().filter(|v| s.in_use(v)).count()).unwrap_or(0));
                            active.to_string()
                        })
                        unit=Signal::derive(move || format!("/ {}", names(&values.get(), VisitorEntry::section()).len()))
                        detail=Signal::derive(move || {
                            let seen = s.status.with(|st| st.as_ref().map(|st| st.visitors.iter().filter(|v| s.in_use(v) || v.last_seen_secs.is_some()).count()).unwrap_or(0));
                            format!("{seen} used since start")
                        })
                        testid="kpi-visitors"
                    />
                </div>
                <div class="md:col-span-2">
                    <Kpi
                        label="Refused handshakes"
                        icon="shield"
                        value=Signal::derive(move || refused_log.with(|l| match (l.front(), l.back()) {
                            (Some(a), Some(b)) => b.1.saturating_sub(a.1).to_string(),
                            _ => "0".into(),
                        }))
                        unit="/min"
                        detail=Signal::derive(move || s.status.with(|st| format!("{} since start", format::count(st.as_ref().map(|st| st.handshakes_refused).unwrap_or(0)))))
                        testid="kpi-refused"
                    />
                </div>
                <div class="col-span-2 grid gap-4 md:col-span-6 xl:col-span-12 min-[1200px]:grid-cols-12" data-testid="overview-row3">
                    <Traffic/>
                    <Attention/>
                </div>
                <section class="col-span-2 rounded-card border border-border bg-surface md:col-span-6 xl:col-span-12" data-testid="overview-tabs">
                    <div class="flex flex-wrap items-center gap-3 border-b border-border px-3 py-2.5">
                        <PillTabs
                            tabs=vec![(ServiceEntry::section(), "Services".into()), (ClientEntry::section(), "Clients".into()), (VisitorEntry::section(), "Visitors".into())]
                            active=tab
                            counts=counts
                            on_change=Callback::new(move |t: &'static str| {
                                tab.set(t);
                                route::set_query("tab", t);
                            })
                        />
                        <span class="ml-auto text-xs text-text-muted">
                            "current rates · "
                            {move || {
                                let t = tab.get();
                                view! { <Link to=Route::List(t.to_string()) class="font-semibold text-accent-ink no-underline hover:underline">{format!("Open {t} →")}</Link> }
                            }}
                        </span>
                    </div>
                    <div role="tabpanel">
                        {move || match entity_of_section(tab.get()) {
                            Some(Entity::Client) => clients_table().into_any(),
                            Some(Entity::Visitor) => visitors_table().into_any(),
                            _ => services_table().into_any(),
                        }}
                    </div>
                </section>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;
    use burrow_admin_proto::status::ClientStatus;

    fn status(online: &[&str], offline: &[&str]) -> Status {
        let mut st: Status = burrow_admin_proto::serde_json::from_value(json!({
            "version": "0.1.0", "uptime_secs": 10, "listening": [], "handshakes_refused": 0, "udp_peers": 0,
            "clients": [], "services": [], "visitors": []
        }))
        .unwrap();
        for (n, on) in online
            .iter()
            .map(|n| (n, true))
            .chain(offline.iter().map(|n| (n, false)))
        {
            st.clients.push(ClientStatus {
                name: n.to_string(),
                connected: on,
                connected_secs: on.then_some(60),
                from: None,
                open: 0,
                idle_channels: 0,
                left_secs_ago: (!on).then_some(172_800),
                local: false,
            });
        }
        st
    }

    #[test]
    fn needs_attention_groups_by_the_offline_client_and_away_lists_the_rest() {
        let v = json!({
            "clients": {"a": {}, "b": {}, "c": {}, "d": {}},
            "services": {
                "aa-partial": {"clients": ["a", "b"], "target": "x:1"},
                "zz-down": {"client": "b", "target": "x:2"},
                "ok": {"client": "a", "target": "x:3"},
                "both": {"clients": ["c", "d"], "target": "x:4"}
            }
        });
        let st = status(&["a"], &["b", "c", "d"]);
        let items = attention(&v, None, Some(&st));
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Client b is offline", "Service both is down"]);
        assert_eq!(
            items[0].detail,
            "zz-down is down; aa-partial runs on 1 of 2 clients · left 2d 0h ago"
        );
        assert_eq!(items[0].tone, Tone::Danger);
        assert_eq!(items[1].detail, "None of its 2 clients is online: c, d.");
        let a = away(&v, Some(&st));
        assert_eq!(
            a.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["b", "c", "d"],
            "every offline client is listed in Clients away"
        );
        assert!(a[1].detail.starts_with("serves both · "), "{}", a[1].detail);
        assert!(attention(&v, None, None).is_empty());
    }

    #[test]
    fn services_healthy_counts_services_whose_client_is_offline() {
        let v = json!({
            "clients": {"a": {}, "b": {}},
            "services": {
                "web": {"client": "a", "target": "x:1"},
                "backup": {"client": "b", "target": "x:2"},
                "both": {"clients": ["a", "b"], "target": "x:3"}
            }
        });
        let st = status(&["a"], &["b"]);
        let p = service_problems(&v, Some(&st));
        assert_eq!(
            p,
            [
                ("backup".to_string(), Problem::Down),
                ("both".to_string(), Problem::Partial)
            ]
        );
        assert_eq!(healthy_detail(&p), "2 with problems");
        assert_eq!(healthy_detail(&p[..1]), "backup is down");
        assert_eq!(healthy_detail(&p[1..]), "both runs on fewer clients");
        assert_eq!(healthy_detail(&[]), "all up");
    }

    #[test]
    fn the_chart_scales_to_mbit_and_keeps_gaps() {
        let mb = 1_000_000.0 / 8.0;
        let a = vec![Some(10.0 * mb), None, Some(50.0 * mb)];
        let b = vec![Some(1.0 * mb), None, Some(2.0 * mb)];
        let d = chart(&a, &b, 60);
        assert_eq!(d.max, 60.0);
        assert_eq!(d.a[1], None);
        assert!((d.a[2].unwrap() - 50.0).abs() < 1e-9);
        assert_eq!(d.y_ticks, ["0", "15", "30", "45", "60"]);
        assert_eq!(d.x_ticks.last().map(String::as_str), Some("now"));
        assert!(d.empty_note.unwrap().contains("3s so far"));
        assert!(chart(&a[..1], &b[..1], 900).a.is_empty());
        let full: Vec<Option<f64>> = vec![Some(mb); 1000];
        let d = chart(&full, &full, 900);
        assert_eq!(d.a.len(), 900);
        assert!(d.empty_note.is_none());
    }
}
