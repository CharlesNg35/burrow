use leptos::prelude::*;

use crate::basic::Tone;
use crate::icon::Icon;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FieldState {
    pub error: Option<String>,
    pub warning: Option<String>,
    pub info: Option<String>,
    pub disabled_reason: Option<String>,
    pub locked_reason: Option<String>,
    pub modified: bool,
}

pub fn tag_class(tone: Tone) -> String {
    format!(
        "inline-flex h-[18px] items-center gap-1 rounded-xs px-1.5 text-[10px] font-bold tracking-wide whitespace-nowrap {}",
        tone.tag()
    )
}

pub fn field_message(tone: Tone, m: impl IntoView + 'static) -> AnyView {
    let (icon, class) = match tone {
        Tone::Danger => ("alert-circle", "text-danger"),
        Tone::Warning => ("alert", "text-warning"),
        Tone::Info | Tone::Accent => ("info", "text-accent-ink"),
        _ => ("info", "text-text-muted"),
    };
    view! {
        <p class=format!("mt-1.5 flex items-start gap-1.5 text-xs {class}") data-tone=tone.name()>
            <Icon name=icon small=true/>
            <span>{m}</span>
        </p>
    }
    .into_any()
}

#[component]
pub fn FieldRow(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    #[prop(optional, into)] help: String,
    #[prop(optional, into)] hide_help: Signal<bool>,
    #[prop(optional, into)] path: String,
    #[prop(optional, into)] schema_path: String,
    #[prop(optional, into)] key_hint: String,
    #[prop(optional, into)] badges: Vec<(String, Tone)>,
    #[prop(optional)] wide: bool,
    #[prop(optional)] table: bool,
    #[prop(optional, into)] aside: Vec<(String, Tone)>,
    #[prop(optional, into)] state: Signal<FieldState>,
    #[prop(optional)] quiet: bool,
    children: Children,
) -> impl IntoView {
    let help_id = format!("{id}-help");
    let described = help_id.clone();
    let has_help = !help.is_empty();
    let class = move || {
        let s = state.get();
        format!(
            "relative min-w-0{}{}",
            if table {
                " grid grid-cols-1 gap-x-4 gap-y-1.5 py-3.5 md:grid-cols-[210px_minmax(0,1fr)_110px]"
            } else if wide {
                " md:col-span-2"
            } else {
                ""
            },
            if s.modified {
                " before:absolute before:top-0 before:bottom-0 before:-left-3 before:w-0.5 before:rounded before:bg-accent"
            } else {
                ""
            }
        )
    };
    let help_line = move || {
        let s = state.get();
        s.error
            .map(|m| field_message(Tone::Danger, m))
            .or(s.disabled_reason.map(|m| field_message(Tone::Neutral, m)))
            .or(s.warning.map(|m| field_message(Tone::Warning, m)))
            .or(s.info.map(|m| field_message(Tone::Info, m)))
    };
    let help_text = move || {
        (has_help
            && !quiet
            && !hide_help.get()
            && state.with(|s| s.error.is_none() && s.disabled_reason.is_none()))
        .then(|| {
            let help = help.clone();
            let id = help_id.clone();
            view! { <p class="mt-1.5 text-xs text-text-muted" id=id>{help}</p> }
        })
    };
    view! {
        <div
            class=class
            data-path=path
            data-schema=schema_path
            data-disabled=move || state.get().disabled_reason.is_some().then_some("true")
            data-modified=move || state.get().modified.then_some("true")
        >
            <div class=if quiet { "sr-only" } else if table { "flex min-w-0 flex-col items-start gap-1 pt-2" } else { "mb-1.5 flex flex-wrap items-center gap-2" }>
                <label class="text-sm font-semibold" for=id.clone() id=format!("{id}-label")>
                    {label}
                </label>
                {(!key_hint.is_empty()).then(|| view! { <span class="font-mono text-xs break-words text-text-faint">{key_hint}</span> })}
                {badges
                    .into_iter()
                    .map(|(text, tone)| view! { <span class=tag_class(tone)>{text}</span> })
                    .collect_view()}
            </div>
            <div class="min-w-0">
                <div
                    class=move || if state.get().disabled_reason.is_some() { "opacity-55" } else { "" }
                    aria-describedby=has_help.then_some(described)
                >
                    {children()}
                </div>
                {help_text}
                <div aria-live="polite">{help_line}</div>
            </div>
            {table.then(|| view! {
                <div class="flex flex-wrap items-start gap-1 pt-2 md:justify-end">
                    {aside.into_iter().map(|(text, tone)| view! { <span class=tag_class(tone)>{text}</span> }).collect_view()}
                </div>
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_and_a_chip_of_one_tone_share_its_colours() {
        for tone in [
            Tone::Neutral,
            Tone::Accent,
            Tone::Success,
            Tone::Warning,
            Tone::Danger,
            Tone::Info,
            Tone::Violet,
        ] {
            assert!(tag_class(tone).ends_with(tone.tag()), "{tone:?}");
            assert!(
                crate::chip_class(tone, false).contains(tone.tag()),
                "{tone:?}"
            );
        }
    }
}
