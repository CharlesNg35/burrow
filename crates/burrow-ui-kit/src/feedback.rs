use leptos::prelude::*;

use crate::basic::{SpinDot, Tone};
use crate::icon::Icon;

fn icon_for(tone: Tone) -> &'static str {
    match tone {
        Tone::Danger => "alert",
        Tone::Warning => "warning",
        Tone::Success => "check",
        Tone::Info | Tone::Accent | Tone::Neutral => "info",
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
            class=format!("mb-3 flex items-start gap-3 rounded-card border p-3 {}", tone.soft())
            role=role
            data-tone=tone.name()
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            <Icon name=icon_for(tone)/>
            <div class="flex-1 text-text">
                <strong class="block">{move || title.get()}</strong>
                <span class="text-sm">{move || message.get()}</span>
            </div>
            <div class="flex gap-2">{children.map(|c| c())}</div>
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
            class=format!("mb-3 rounded-card border border-l-4 p-3 {} {}", tone.soft(), tone.bar())
            data-tone=tone.name()
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            <div class="mb-1 flex items-center gap-2 font-semibold">
                <Icon name=icon_for(tone)/>
                {title}
            </div>
            <div class="text-text">{children()}</div>
        </div>
    }
}

#[component]
pub fn EmptyState(
    #[prop(into)] title: String,
    #[prop(optional, into)] message: String,
    #[prop(optional)] testid: &'static str,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div
            class="rounded-card border border-dashed border-border px-6 py-10 text-center"
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            <div class="text-md font-semibold">{title}</div>
            <p class="mx-auto mt-1 mb-4 max-w-prose text-text-muted">{message}</p>
            {children.map(|c| c())}
        </div>
    }
}

#[component]
pub fn Skeleton(#[prop(optional)] lines: usize) -> impl IntoView {
    view! {
        <div class="space-y-3" aria-busy="true" aria-label="Loading">
            {(0..lines.max(1))
                .map(|_| view! { <div class="h-4 animate-pulse rounded bg-surface-2"></div> })
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
