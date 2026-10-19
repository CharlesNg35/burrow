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
                "relative flex min-w-[130px] max-w-[240px] shrink-0 flex-col gap-1 rounded-hop border p-3.5 {}{}",
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
        <div class="flex min-w-[56px] flex-1 flex-col justify-center px-1.5 text-center" data-edge=format!("{:?}", e.state).to_lowercase()>
            <span class=format!("font-mono text-[11px] leading-tight {label}")>{e.above.clone()}</span>
            <div class="my-1 flex items-center">
                <div class=format!("flex-1 {line}")></div>
                <div class=format!("size-0 border-y-[4px] border-l-[7px] border-y-transparent {head}")></div>
            </div>
            <span class=format!("text-[11px] leading-tight {}", if e.state == EdgeState::Live { "text-text-faint" } else { label })>{e.below.clone()}</span>
        </div>
    }
}

#[component]
pub fn PathDiagram(
    #[prop(into)] data: Signal<PathData>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    view! {
        <div
            class="mb-[22px] flex items-stretch overflow-hidden rounded-[14px] border border-border bg-surface px-5 pt-6 pb-5 max-sm:flex-col max-sm:gap-2"
            role="group"
            aria-label=move || data.get().summary
            data-testid=(!testid.is_empty()).then_some(testid)
        >
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
    }
}
