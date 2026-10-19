use leptos::prelude::*;

use crate::icon::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
    Info,
    Udp,
}

impl Tone {
    pub fn name(self) -> &'static str {
        match self {
            Tone::Neutral => "neutral",
            Tone::Accent => "accent",
            Tone::Success => "success",
            Tone::Warning => "warning",
            Tone::Danger => "danger",
            Tone::Info => "info",
            Tone::Udp => "udp",
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Tone::Neutral => "border border-border bg-surface-3 text-text-muted",
            Tone::Accent | Tone::Info => "bg-accent-weak text-accent-ink",
            Tone::Success => "bg-success-weak text-success",
            Tone::Warning => "bg-warning-weak text-warning",
            Tone::Danger => "bg-danger-weak text-danger",
            Tone::Udp => "bg-udp-weak text-udp",
        }
    }

    pub fn soft(self) -> &'static str {
        match self {
            Tone::Neutral => "border-border bg-surface-2 text-text",
            Tone::Accent | Tone::Info => "border-accent/40 bg-accent-weak text-co-info",
            Tone::Success => "border-success/40 bg-success-weak text-co-success",
            Tone::Warning => "border-warning/40 bg-warning-weak text-co-warning",
            Tone::Danger => "border-danger/40 bg-danger-weak text-co-danger",
            Tone::Udp => "border-udp/40 bg-udp-weak text-udp",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Tone::Neutral => "text-text-muted",
            Tone::Accent | Tone::Info => "text-accent-ink",
            Tone::Success => "text-success",
            Tone::Warning => "text-warning",
            Tone::Danger => "text-danger",
            Tone::Udp => "text-udp",
        }
    }

    pub fn dot(self) -> &'static str {
        match self {
            Tone::Success => "bg-success ring-3 ring-success-weak",
            Tone::Warning => "bg-warning ring-3 ring-warning-weak",
            Tone::Danger => "bg-danger ring-3 ring-danger-weak",
            Tone::Accent | Tone::Info => "bg-accent ring-3 ring-accent-weak",
            Tone::Neutral | Tone::Udp => "bg-transparent ring-[1.5px] ring-text-faint ring-inset",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Tone::Danger => "alert-circle",
            Tone::Warning => "alert",
            Tone::Success => "check",
            _ => "info",
        }
    }
}

pub fn chip_class(tone: Tone, mono: bool) -> String {
    format!(
        "inline-flex items-center gap-1 rounded-sm px-2 py-0.5 text-sm font-semibold no-underline hover:underline {}{}",
        tone.tag(),
        if mono { " font-mono" } else { "" }
    )
}

pub const LINK: &str = "text-accent-ink underline-offset-2 hover:underline";
pub const MONO: &str = "font-mono";
pub const LABEL: &str = "text-2xs font-bold uppercase tracking-[0.08em] text-text-faint";

pub fn button_class(variant: &str, small: bool) -> String {
    let look = match variant {
        "primary" => "border-accent bg-accent text-on-accent hover:brightness-110",
        "danger" => "border-danger bg-danger text-danger-ink hover:brightness-110",
        "danger-pending" => "border-danger/45 bg-transparent text-danger",
        "tinted" => "border-accent-line bg-accent-weak text-accent-ink hover:brightness-105",
        "ghost" => {
            "border-transparent bg-transparent text-text-muted hover:bg-surface-3 hover:text-text"
        }
        "ghost-danger" => "border-transparent bg-transparent text-danger hover:bg-danger-weak",
        _ => "border-border-strong bg-surface text-text hover:bg-surface-3 dark:bg-surface-2",
    };
    let size = if small {
        "h-[26px] px-[9px] text-xs"
    } else {
        "h-9 px-3 text-sm"
    };
    format!(
        "inline-flex items-center justify-center gap-1.5 rounded-input border font-semibold whitespace-nowrap cursor-pointer no-underline transition-colors disabled:cursor-not-allowed disabled:opacity-55 {look} {size}"
    )
}

#[component]
pub fn Button(
    #[prop(into)] label: Signal<String>,
    #[prop(optional)] variant: &'static str,
    #[prop(optional)] small: bool,
    #[prop(optional)] icon: &'static str,
    #[prop(optional)] kbd: &'static str,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] reason: Signal<String>,
    #[prop(optional, into)] busy: Signal<bool>,
    #[prop(optional)] testid: &'static str,
    #[prop(into)] on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class=move || button_class(if variant == "danger" && disabled.get() { "danger-pending" } else { variant }, small)
            disabled=move || disabled.get() || busy.get()
            title=move || Some(reason.get()).filter(|r| !r.is_empty())
            aria-busy=move || busy.get().then_some("true")
            data-testid=(!testid.is_empty()).then_some(testid)
            on:click=move |_| on_click.run(())
        >
            {move || busy.get().then(|| view! { <SpinDot/> })}
            {(!icon.is_empty()).then(|| view! { <Icon name=icon/> })}
            <span>{move || label.get()}</span>
            {(!kbd.is_empty()).then(|| view! { <Kbd text=kbd/> })}
        </button>
    }
}

#[component]
pub fn SpinDot() -> impl IntoView {
    view! {
        <span
            class="inline-block size-3.5 animate-spin rounded-full border-2 border-current border-r-transparent"
            aria-hidden="true"
        ></span>
    }
}

#[component]
pub fn Badge(
    #[prop(into)] text: Signal<String>,
    #[prop(optional)] tone: Tone,
    #[prop(optional)] icon: &'static str,
    #[prop(optional)] mono: bool,
    #[prop(optional, into)] title: Option<String>,
) -> impl IntoView {
    view! {
        <span
            class=format!(
                "inline-flex h-5 items-center gap-1 rounded-[5px] px-[7px] text-[11px] font-semibold whitespace-nowrap {}{}",
                tone.tag(),
                if mono { " font-mono" } else { "" }
            )
            title=title
            data-tone=tone.name()
        >
            {(!icon.is_empty()).then(|| view! { <Icon name=icon small=true/> })}
            {move || text.get()}
        </span>
    }
}

#[component]
pub fn Chip(
    #[prop(into)] text: String,
    #[prop(optional)] mono: bool,
    #[prop(optional)] tone: Tone,
) -> impl IntoView {
    view! { <span class=chip_class(tone, mono) data-tone=tone.name()>{text}</span> }
}

#[component]
pub fn StatusDot(
    #[prop(into)] on: Signal<bool>,
    #[prop(into)] text: Signal<String>,
    #[prop(optional, into)] tone: Option<Signal<Tone>>,
) -> impl IntoView {
    let tone = move || {
        tone.map(|t| t.get()).unwrap_or(if on.get() {
            Tone::Success
        } else {
            Tone::Neutral
        })
    };
    view! {
        <span class="inline-flex items-center gap-2 text-sm whitespace-nowrap" data-on=move || on.get().to_string()>
            <span class=move || format!("size-2 shrink-0 rounded-full {}", tone().dot()) aria-hidden="true"></span>
            <span>{move || text.get()}</span>
        </span>
    }
}

#[component]
pub fn Kpi(
    #[prop(into)] label: String,
    #[prop(into)] value: Signal<String>,
    #[prop(optional, into)] unit: Signal<String>,
    #[prop(optional, into)] detail: Signal<String>,
    #[prop(optional)] icon: &'static str,
    #[prop(optional, into)] tone: Signal<Tone>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    view! {
        <div
            class="rounded-card border border-border bg-surface px-[18px] py-4"
            data-testid=(!testid.is_empty()).then_some(testid)
            data-tone=move || tone.get().name()
        >
            <div class="flex items-center gap-1.5 text-xs text-text-muted">
                {(!icon.is_empty()).then(|| view! { <Icon name=icon small=true/> })}
                {label}
            </div>
            <div class="mt-1.5 text-[26px] leading-tight font-bold tabular-nums">
                {move || value.get()}
                <small class="ml-1 text-[13px] font-medium text-text-muted">{move || unit.get()}</small>
            </div>
            <div class="mt-0.5 text-xs text-text-muted">{move || detail.get()}</div>
        </div>
    }
}

#[component]
pub fn Card(
    #[prop(optional, into)] title: Option<String>,
    #[prop(optional, into)] sub: Option<String>,
    #[prop(optional)] testid: &'static str,
    #[prop(optional, into)] class: String,
    #[prop(optional)] flush: bool,
    #[prop(optional)] head: Option<ChildrenFn>,
    children: Children,
) -> impl IntoView {
    let has_head = title.is_some() || head.is_some();
    let top = has_head.then(|| {
        view! {
            <div class="flex min-h-12 items-center gap-3 border-b border-border px-4 py-3">
                <h2 class="text-[13.5px] font-bold">{title.clone()}</h2>
                {sub.clone().map(|s| view! { <span class="text-xs text-text-muted">{s}</span> })}
                <div class="ml-auto flex items-center gap-2 text-xs text-text-muted">{head.as_ref().map(|h| h())}</div>
            </div>
        }
    });
    view! {
        <section
            class=format!("rounded-card border border-border bg-surface {class}")
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            {top}
            <div class=if flush { "" } else { "p-4" }>{children()}</div>
        </section>
    }
}

#[component]
pub fn PageHeader(
    #[prop(into)] title: Signal<String>,
    #[prop(optional, into)] subtitle: Signal<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <header class="mb-[22px] flex flex-wrap items-start justify-between gap-3">
            <div class="min-w-0">
                <h1 class="text-2xl font-bold tracking-[-0.015em]">{move || title.get()}</h1>
                <p class="mt-1 text-sm text-text-muted">{move || subtitle.get()}</p>
            </div>
            <div class="flex items-center gap-2">{children.map(|c| c())}</div>
        </header>
    }
}

#[component]
pub fn Tabs(
    tabs: Vec<(&'static str, String)>,
    #[prop(into)] active: RwSignal<&'static str>,
    #[prop(optional)] counts: Option<Signal<Vec<String>>>,
) -> impl IntoView {
    view! {
        <div class="mb-4 flex gap-1 border-b border-border" role="tablist">
            {tabs
                .into_iter()
                .enumerate()
                .map(|(i, (key, label))| {
                    view! {
                        <button
                            type="button"
                            role="tab"
                            class="-mb-px flex items-center gap-1 border-b-2 border-transparent px-3 py-2.5 text-[13px] font-semibold text-text-muted hover:text-text aria-selected:border-accent aria-selected:text-text"
                            aria-selected=move || (active.get() == key).to_string()
                            data-tab=key
                            on:click=move |_| active.set(key)
                        >
                            {label}
                            {counts.map(|c| view! { <span class="text-[11px] text-text-faint">{move || c.get().get(i).cloned().unwrap_or_default()}</span> })}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
pub fn PillTabs(
    tabs: Vec<(&'static str, String)>,
    #[prop(into)] active: Signal<&'static str>,
    #[prop(into)] on_change: Callback<&'static str>,
    #[prop(optional)] counts: Option<Signal<Vec<String>>>,
) -> impl IntoView {
    view! {
        <div class="flex gap-1" role="tablist">
            {tabs
                .into_iter()
                .enumerate()
                .map(|(i, (key, label))| {
                    view! {
                        <button
                            type="button"
                            role="tab"
                            class="flex h-[30px] items-center gap-1.5 rounded-md px-3 text-[13px] font-semibold text-text-muted aria-selected:bg-surface aria-selected:text-text aria-selected:shadow-sm aria-selected:ring-1 aria-selected:ring-border aria-selected:ring-inset dark:aria-selected:bg-surface-3"
                            aria-selected=move || (active.get() == key).to_string()
                            data-tab=key
                            on:click=move |_| on_change.run(key)
                        >
                            {label}
                            {counts.map(|c| view! { <span class="text-[11px] text-text-faint">{move || c.get().get(i).cloned().unwrap_or_default()}</span> })}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
pub fn Table(
    head: Vec<&'static str>,
    #[prop(optional)] testid: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table
                class="w-full border-collapse text-left text-base [&_td]:h-[46px] [&_td]:border-b [&_td]:border-rowline [&_td]:px-4 [&_td]:whitespace-nowrap [&_tbody_th]:h-[46px] [&_tbody_th]:border-b [&_tbody_th]:border-rowline [&_tbody_th]:px-4 [&_tbody_th]:font-semibold [&_tbody_tr:last-child>*]:border-b-0 [&_tbody_tr:hover]:bg-surface-2 dark:[&_tbody_tr:hover]:bg-surface-3/50"
                data-testid=(!testid.is_empty()).then_some(testid)
            >
                <thead>
                    <tr>
                        {head
                            .into_iter()
                            .map(|h| {
                                let right = h.starts_with('>');
                                let shrink = h.starts_with('_');
                                let text = h.trim_start_matches(['>', '_']);
                                view! {
                                    <th
                                        scope="col"
                                        class=format!(
                                            "border-b border-border px-4 py-[9px] text-[11px] font-bold tracking-[0.07em] whitespace-nowrap text-text-faint uppercase {}{}",
                                            if right { "text-right" } else { "text-left" },
                                            if shrink { " w-px" } else { "" }
                                        )
                                    >
                                        {text}
                                    </th>
                                }
                            })
                            .collect_view()}
                    </tr>
                </thead>
                <tbody>{children()}</tbody>
            </table>
        </div>
    }
}

#[component]
pub fn Facts(#[prop(optional)] testid: &'static str, children: Children) -> impl IntoView {
    view! {
        <dl
            class="grid grid-cols-[repeat(auto-fill,minmax(12rem,1fr))] gap-x-6 gap-y-3 [&_dt]:text-xs [&_dt]:text-text-muted [&_dd]:mt-0.5 [&_dd]:break-words"
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            {children()}
        </dl>
    }
}

#[component]
pub fn Fact(
    #[prop(into)] label: String,
    #[prop(optional)] mono: bool,
    children: Children,
) -> impl IntoView {
    view! {
        <div>
            <dt>{label}</dt>
            <dd class=if mono { "font-mono" } else { "tabular-nums" }>{children()}</dd>
        </div>
    }
}

#[component]
pub fn Stats(#[prop(optional)] testid: &'static str, children: Children) -> impl IntoView {
    view! {
        <div
            class="mb-[22px] grid grid-cols-2 overflow-hidden rounded-card border border-border bg-surface md:grid-cols-4 [&>*]:border-border [&>*]:px-4 [&>*]:py-3.5 [&>*:not(:last-child)]:border-r"
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            {children()}
        </div>
    }
}

#[component]
pub fn Stat(
    #[prop(into)] label: String,
    #[prop(into)] value: Signal<String>,
    #[prop(optional, into)] unit: Signal<String>,
    #[prop(optional, into)] detail: Signal<String>,
) -> impl IntoView {
    let key = label.to_lowercase();
    view! {
        <div>
            <div class="text-xs text-text-muted">{label}</div>
            <div class="mt-1 text-xl font-bold tabular-nums" data-stat=key>
                {move || value.get()}
                <small class="ml-1 text-xs font-medium text-text-muted">{move || unit.get()}</small>
            </div>
            <div class="text-xs text-text-muted">{move || detail.get()}</div>
        </div>
    }
}

#[component]
pub fn Muted(#[prop(optional)] testid: &'static str, children: Children) -> impl IntoView {
    view! { <span class="text-text-muted" data-testid=(!testid.is_empty()).then_some(testid)>{children()}</span> }
}

#[component]
pub fn Mono(children: Children) -> impl IntoView {
    view! { <span class="font-mono">{children()}</span> }
}

#[component]
pub fn Kbd(#[prop(into)] text: String) -> impl IntoView {
    view! { <kbd class="rounded-xs border border-border-strong px-1 font-mono text-2xs leading-4 text-text-muted">{text}</kbd> }
}
