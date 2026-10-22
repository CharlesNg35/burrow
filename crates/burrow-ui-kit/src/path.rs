use leptos::prelude::*;

use crate::basic::Tone;
use crate::icon::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HopState {
    #[default]
    Normal,
    Current,
    Bad,
    Warn,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Hop {
    pub icon: &'static str,
    pub head: String,
    pub value: String,
    pub mono: bool,
    pub dot: Option<Tone>,
    pub unit: String,
    pub state: HopState,
    pub badge: Option<(String, Tone)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeState {
    #[default]
    Live,
    Dead,
    Error,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Edge {
    pub above: String,
    pub below: String,
    pub state: EdgeState,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PathData {
    pub hops: Vec<Hop>,
    pub edges: Vec<Edge>,
    pub summary: String,
}

fn hop_class(s: HopState) -> &'static str {
    match s {
        HopState::Normal => "border-border bg-hop",
        HopState::Current => "border-accent-line bg-hop ring-3 ring-accent-weak",
        HopState::Bad => "border-danger/55 bg-danger-weak",
        HopState::Warn => "border-warning/50 bg-hop",
        HopState::Added => "border-dashed border-success bg-success-weak",
        HopState::Removed => "border-dashed border-danger bg-danger-weak",
    }
}

fn badge_class(t: Tone) -> &'static str {
    match t {
        Tone::Danger => "bg-danger text-white dark:text-danger-ink",
        Tone::Warning => "bg-warning text-warning-ink",
        Tone::Success => "bg-success text-white dark:text-[#04170D]",
        _ => "bg-surface-3 text-text-muted ring-1 ring-border",
    }
}

fn hop_view(h: Hop) -> impl IntoView {
    let removed = h.state == HopState::Removed;
    view! {
        <div
            class=format!(
                "relative flex min-w-[104px] max-w-[240px] shrink flex-col gap-1 rounded-hop border p-3.5 {}{}",
                hop_class(h.state),
                if h.badge.is_some() { " pt-5" } else { "" }
            )
            data-hop=h.head.to_lowercase()
            data-state=format!("{:?}", h.state).to_lowercase()
        >
            {h.badge.clone().map(|(text, tone)| view! {
                <span class=format!("absolute -top-2.5 right-2.5 inline-flex h-[19px] items-center gap-1 rounded-full px-[7px] text-2xs font-bold whitespace-nowrap {}", badge_class(tone)) data-badge=text.clone()>
                    <Icon name=if tone == Tone::Success { "check" } else { "alert-circle" } small=true/>
                    {text.clone()}
                </span>
            })}
            <div class="flex items-center gap-1.5 text-[11px] font-bold tracking-[0.07em] text-text-muted uppercase">
                <Icon name=h.icon/>
                {h.head.clone()}
            </div>
            <div class=format!(
                "flex min-w-0 items-center gap-1.5 font-semibold {}{}",
                if h.mono { "font-mono text-[13.5px]" } else { "text-md" },
                if removed { " line-through" } else { "" }
            )>
                {h.dot.map(|t| view! { <span class=format!("size-2 shrink-0 rounded-full {}", t.dot()) aria-hidden="true"></span> })}
                <span class="truncate" title=h.value.clone()>{h.value.clone()}</span>
            </div>
            <div class="truncate text-xs text-text-muted">{h.unit.clone()}</div>
        </div>
    }
}

fn edge_view(e: Edge) -> impl IntoView {
    let (line, label) = match e.state {
        EdgeState::Live => (
            "h-0.5 bg-gradient-to-r from-accent-line to-accent",
            "text-text-muted",
        ),
        EdgeState::Dead => (
            "border-t-2 border-dashed border-text-faint",
            "text-text-faint",
        ),
        EdgeState::Error | EdgeState::Removed => {
            ("border-t-2 border-dashed border-danger", "text-danger")
        }
        EdgeState::Added => ("border-t-2 border-dashed border-success", "text-success"),
    };
    let head = match e.state {
        EdgeState::Live => "border-l-accent",
        EdgeState::Dead => "border-l-text-faint",
        EdgeState::Error | EdgeState::Removed => "border-l-danger",
        EdgeState::Added => "border-l-success",
    };
    view! {
        <div class="flex min-w-[36px] flex-1 flex-col justify-center px-1.5 text-center" data-edge=format!("{:?}", e.state).to_lowercase()>
            <span class=format!("font-mono text-[11px] leading-tight {label}")>{e.above.clone()}</span>
            <div class="my-1 flex items-center">
                <div class=format!("flex-1 {line}")></div>
                <div class=format!("size-0 border-y-[4px] border-l-[7px] border-y-transparent {head}")></div>
            </div>
            <span class=format!("text-[11px] leading-tight {}", if e.state == EdgeState::Live { "text-text-faint" } else { label })>{e.below.clone()}</span>
        </div>
    }
}

fn compact_view(i: usize, h: Hop, next: Option<Edge>) -> impl IntoView {
    let tone = match h.state {
        HopState::Bad | HopState::Removed => Tone::Danger,
        HopState::Warn => Tone::Warning,
        _ => h.dot.unwrap_or(Tone::Success),
    };
    let line = next.map(|e| match e.state {
        EdgeState::Live => "border-l-2 border-accent-line",
        EdgeState::Dead => "border-l-2 border-dashed border-text-faint",
        EdgeState::Error | EdgeState::Removed => "border-l-2 border-dashed border-danger",
        EdgeState::Added => "border-l-2 border-dashed border-success",
    });
    let sub = [h.value.clone(), h.unit.clone()]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let head = {
        let mut c = h.head.chars();
        c.next()
            .map(|f| {
                f.to_uppercase()
                    .chain(c.flat_map(char::to_lowercase))
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    view! {
        <li class=format!("relative pl-6 {}", if line.is_some() { "pb-3.5" } else { "" }) data-compact-hop=i>
            <span class=format!("absolute top-[5px] left-0 size-2.5 rounded-full {}", tone.dot()) aria-hidden="true"></span>
            {line.map(|l| view! { <span class=format!("absolute top-[19px] bottom-0 left-[4px] {l}") aria-hidden="true"></span> })}
            <div class="flex flex-wrap items-center gap-2 text-sm font-semibold">
                {head}
                {h.badge.clone().map(|(text, tone)| view! {
                    <span class=format!("inline-flex h-[19px] items-center rounded-full px-[7px] text-2xs font-bold whitespace-nowrap {}", badge_class(tone))>{text}</span>
                })}
            </div>
            <div class=format!("truncate text-xs text-text-muted{}", if h.state == HopState::Removed { " line-through" } else { "" })>{sub}</div>
        </li>
    }
}

#[component]
pub fn PathDiagram(
    #[prop(into)] data: Signal<PathData>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    view! {
        <div
            role="group"
            aria-label=move || data.get().summary
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            <div class="mb-[22px] flex items-stretch overflow-hidden rounded-[14px] border border-border bg-surface px-5 pt-6 pb-5 max-xl:hidden">
                {move || {
                    let d = data.get();
                    let mut edges = d.edges.into_iter();
                    d.hops
                        .into_iter()
                        .enumerate()
                        .map(|(i, h)| {
                            let edge = (i > 0).then(|| edges.next().unwrap_or_default());
                            view! {
                                {edge.map(edge_view)}
                                {hop_view(h)}
                            }
                        })
                        .collect_view()
                }}
            </div>
            <ol class="mb-4 rounded-[14px] border border-border bg-surface p-4 xl:hidden">
                {move || {
                    let d = data.get();
                    let n = d.hops.len();
                    let edges = d.edges;
                    d.hops
                        .into_iter()
                        .enumerate()
                        .map(|(i, h)| {
                            let next = (i + 1 < n).then(|| edges.get(i).cloned().unwrap_or_default());
                            compact_view(i, h, next)
                        })
                        .collect_view()
                }}
            </ol>
        </div>
    }
}
