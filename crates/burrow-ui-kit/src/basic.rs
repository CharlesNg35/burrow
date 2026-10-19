use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
    Info,
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
        }
    }

    pub fn soft(self) -> &'static str {
        match self {
            Tone::Neutral => "bg-surface-2 text-text border-border",
            Tone::Accent => "bg-accent-weak text-accent border-accent/40",
            Tone::Success => "bg-success/10 text-success border-success/40",
            Tone::Warning => "bg-warning/10 text-warning border-warning/40",
            Tone::Danger => "bg-danger/10 text-danger border-danger/40",
            Tone::Info => "bg-info/10 text-info border-info/40",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Tone::Neutral => "text-text",
            Tone::Accent => "text-accent",
            Tone::Success => "text-success",
            Tone::Warning => "text-warning",
            Tone::Danger => "text-danger",
            Tone::Info => "text-info",
        }
    }

    pub fn bar(self) -> &'static str {
        match self {
            Tone::Neutral => "border-l-border",
            Tone::Accent => "border-l-accent",
            Tone::Success => "border-l-success",
            Tone::Warning => "border-l-warning",
            Tone::Danger => "border-l-danger",
            Tone::Info => "border-l-info",
        }
    }
}

pub fn chip_class(tone: Tone, mono: bool) -> String {
    format!(
        "inline-flex items-center gap-1 rounded-input border px-2 py-0.5 text-xs no-underline hover:underline {}{}",
        tone.soft(),
        if mono { " font-mono" } else { "" }
    )
}

pub const LINK: &str = "text-accent underline-offset-2 hover:underline";

pub fn button_class(variant: &str, small: bool) -> String {
    let look = match variant {
        "primary" => "bg-accent text-white border-accent hover:brightness-110",
        "danger" => "bg-danger text-white border-danger hover:brightness-110",
        "ghost" => "bg-transparent text-text border-transparent hover:bg-surface-2",
        _ => "bg-surface text-text border-border hover:bg-surface-2",
    };
    let size = if small {
        "h-7 px-2.5 text-sm"
    } else {
        "h-9 px-3.5 text-base"
    };
    format!(
        "inline-flex items-center justify-center gap-1.5 rounded-input border font-medium whitespace-nowrap cursor-pointer no-underline transition-colors disabled:cursor-not-allowed disabled:opacity-55 {look} {size}"
    )
}

#[component]
pub fn Button(
    #[prop(into)] label: Signal<String>,
    #[prop(optional)] variant: &'static str,
    #[prop(optional)] small: bool,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] reason: Signal<String>,
    #[prop(optional, into)] busy: Signal<bool>,
    #[prop(optional)] testid: &'static str,
    #[prop(into)] on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class=button_class(variant, small)
            disabled=move || disabled.get() || busy.get()
            title=move || reason.get()
            aria-busy=move || busy.get().then_some("true")
            data-testid=(!testid.is_empty()).then_some(testid)
            on:click=move |_| on_click.run(())
        >
            {move || busy.get().then(|| view! { <SpinDot/> })}
            <span>{move || label.get()}</span>
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
    #[prop(optional, into)] title: Option<String>,
) -> impl IntoView {
    view! {
        <span
            class=format!("inline-flex items-center rounded-full border px-2 py-px text-xs font-medium whitespace-nowrap {}", tone.soft())
            title=title
            data-tone=tone.name()
        >
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
) -> impl IntoView {
    view! {
        <span class="inline-flex items-center gap-1.5 text-sm whitespace-nowrap" data-on=move || on.get().to_string()>
            <span
                class=move || {
                    if on.get() {
                        "size-2 rounded-full bg-success"
                    } else {
                        "size-2 rounded-full border border-text-muted bg-transparent"
                    }
                }
                aria-hidden="true"
            ></span>
            <span>{move || text.get()}</span>
        </span>
    }
}

#[component]
pub fn Kpi(
    #[prop(into)] label: String,
    #[prop(into)] value: Signal<String>,
    #[prop(optional, into)] detail: Signal<String>,
    #[prop(optional, into)] tone: Signal<Tone>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    view! {
        <div
            class=move || format!("rounded-card border border-border border-l-4 bg-surface p-4 shadow-card {}", tone.get().bar())
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            <div class="text-sm text-text-muted">{label}</div>
            <div class="mt-1 text-lg font-semibold tabular-nums">{move || value.get()}</div>
            <div class="mt-0.5 text-xs text-text-muted">{move || detail.get()}</div>
        </div>
    }
}

#[component]
pub fn Card(
    #[prop(optional, into)] title: Option<String>,
    #[prop(optional)] testid: &'static str,
    children: Children,
) -> impl IntoView {
    let head = title.map(|t| view! { <h2 class="mb-3 text-md font-semibold">{t}</h2> });
    view! {
        <section
            class="mb-4 rounded-card border border-border bg-surface p-4 shadow-card"
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            {head}
            {children()}
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
        <header class="mb-4 flex flex-wrap items-end justify-between gap-3">
            <div>
                <h1 class="text-xl font-semibold">{move || title.get()}</h1>
                <p class="text-sm text-text-muted">{move || subtitle.get()}</p>
            </div>
            <div class="flex gap-2">{children.map(|c| c())}</div>
        </header>
    }
}

#[component]
pub fn Tabs(
    tabs: Vec<(&'static str, String)>,
    #[prop(into)] active: RwSignal<&'static str>,
) -> impl IntoView {
    view! {
        <div class="mb-4 flex gap-1 border-b border-border" role="tablist">
            {tabs
                .into_iter()
                .map(|(key, label)| {
                    view! {
                        <button
                            type="button"
                            role="tab"
                            class="-mb-px border-b-2 border-transparent px-3 py-2 text-sm aria-selected:border-accent aria-selected:font-semibold"
                            aria-selected=move || (active.get() == key).to_string()
                            data-tab=key
                            on:click=move |_| active.set(key)
                        >
                            {label}
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
                class="w-full border-collapse text-left text-base [&_td]:h-9 [&_td]:border-t [&_td]:border-border [&_td]:px-3 [&_th]:px-3 [&_tbody_th]:h-9 [&_tbody_th]:border-t [&_tbody_th]:border-border [&_tbody_th]:font-normal [&_tbody_tr:hover]:bg-surface-2"
                data-testid=(!testid.is_empty()).then_some(testid)
            >
                <thead class="bg-surface-2 text-sm text-text-muted">
                    <tr>{head.into_iter().map(|h| view! { <th scope="col" class="h-8 font-medium">{h}</th> }).collect_view()}</tr>
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
            class="grid grid-cols-[repeat(auto-fill,minmax(12rem,1fr))] gap-x-6 gap-y-3 [&_dt]:text-sm [&_dt]:text-text-muted [&_dd]:mt-0.5 [&_dd]:break-words"
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
            <dd class=if mono { "font-mono text-sm" } else { "tabular-nums" }>{children()}</dd>
        </div>
    }
}

#[component]
pub fn Muted(#[prop(optional)] testid: &'static str, children: Children) -> impl IntoView {
    view! { <span class="text-text-muted" data-testid=(!testid.is_empty()).then_some(testid)>{children()}</span> }
}

#[component]
pub fn Mono(children: Children) -> impl IntoView {
    view! { <span class="font-mono text-sm">{children()}</span> }
}

#[component]
pub fn Kbd(#[prop(into)] text: String) -> impl IntoView {
    view! { <kbd class="rounded border border-border bg-surface-2 px-1 font-mono text-xs">{text}</kbd> }
}
