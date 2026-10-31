use leptos::prelude::*;

use crate::basic::{SpinDot, Tone};
use crate::icon::Icon;

fn banner_tone(tone: Tone) -> &'static str {
    match tone {
        Tone::Danger => {
            "border-danger/30 bg-danger-weak text-co-danger dark:border-border dark:bg-surface dark:text-text"
        }
        Tone::Warning => "border-warning/30 bg-warning-weak text-co-warning",
        Tone::Success => "border-success/30 bg-success-weak text-co-success",
        _ => "border-accent/30 bg-accent-weak text-co-info",
    }
}

pub const DANGER_CARD: &str = "border border-l-[3px] border-border border-l-danger bg-danger-weak/50 text-text dark:bg-surface";

fn callout_tone(tone: Tone) -> &'static str {
    match tone {
        Tone::Danger => DANGER_CARD,
        t => banner_tone(t),
    }
}

#[component]
pub fn Banner(
    tone: Tone,
    #[prop(into)] title: Signal<String>,
    #[prop(optional, into)] message: Signal<String>,
    #[prop(optional)] testid: &'static str,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let role = if matches!(tone, Tone::Danger) {
        "alert"
    } else {
        "status"
    };
    view! {
        <div
            class=format!("flex flex-wrap items-center gap-x-3 gap-y-1 border-b px-5 py-[9px] text-[13px] {}", banner_tone(tone))
            role=role
            data-tone=tone.name()
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            <span class=tone.text()><Icon name=tone.icon()/></span>
            <strong class="font-semibold">{move || title.get()}</strong>
            <span>{move || message.get()}</span>
            <div class="ml-auto flex gap-2">{children.map(|c| c())}</div>
        </div>
    }
}

#[component]
pub fn Callout(
    tone: Tone,
    #[prop(into)] title: String,
    #[prop(optional)] testid: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <div
            class=format!("rounded-[10px] border px-3.5 py-3 text-[13px] {}", callout_tone(tone))
            data-tone=tone.name()
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            <div class="mb-1 flex items-center gap-2 font-semibold">
                <span class=tone.text()><Icon name=tone.icon()/></span>
                {title}
            </div>
            <div>{children()}</div>
        </div>
    }
}

#[component]
pub fn EmptyState(
    #[prop(into)] title: String,
    #[prop(optional, into)] message: String,
    #[prop(optional)] icon: &'static str,
    #[prop(optional)] testid: &'static str,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="px-6 py-10 text-center" data-testid=(!testid.is_empty()).then_some(testid)>
            {(!icon.is_empty()).then(|| view! { <div class="mb-2 flex justify-center text-text-faint"><Icon name=icon large=true/></div> })}
            <div class="text-md font-semibold">{title}</div>
            <p class="mx-auto mt-1 mb-4 max-w-prose text-sm text-text-muted">{message}</p>
            {children.map(|c| view! { <div class="flex flex-wrap items-center justify-center gap-2">{c()}</div> })}
        </div>
    }
}

#[component]
pub fn Skeleton(#[prop(optional)] lines: usize) -> impl IntoView {
    view! {
        <div class="space-y-3 p-4" aria-busy="true" aria-label="Loading">
            {(0..lines.max(1))
                .map(|_| view! { <div class="h-4 animate-pulse rounded bg-surface-3"></div> })
                .collect_view()}
        </div>
    }
}

#[component]
pub fn Spinner(#[prop(optional, into)] label: String) -> impl IntoView {
    view! {
        <span class="inline-flex items-center gap-2 text-text-muted" role="status">
            <SpinDot/>
            <span>{label}</span>
        </span>
    }
}
