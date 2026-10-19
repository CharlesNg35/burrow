use leptos::prelude::*;

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

#[component]
pub fn FieldRow(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    #[prop(optional, into)] help: String,
    #[prop(optional, into)] path: String,
    #[prop(optional, into)] schema_path: String,
    #[prop(optional, into)] badges: Vec<(String, &'static str)>,
    #[prop(optional, into)] state: Signal<FieldState>,
    children: Children,
) -> impl IntoView {
    let help_id = format!("{id}-help");
    let described = help_id.clone();
    let has_help = !help.is_empty();
    let class = move || {
        let s = state.get();
        let bar = if s.error.is_some() {
            "border-l-danger"
        } else if s.modified {
            "border-l-accent"
        } else if s.warning.is_some() {
            "border-l-warning"
        } else {
            "border-l-transparent"
        };
        let muted = if s.disabled_reason.is_some() {
            " opacity-60"
        } else {
            ""
        };
        format!(
            "grid grid-cols-1 gap-x-6 gap-y-1 border-l-3 py-3 pl-3 md:grid-cols-[16rem_1fr] {bar}{muted}"
        )
    };
    view! {
        <div
            class=class
            data-path=path
            data-schema=schema_path
            data-disabled=move || state.get().disabled_reason.is_some().then_some("true")
        >
            <div>
                <label class="font-medium" for=id.clone() id=format!("{id}-label")>
                    {label}
                </label>
                <div class="mt-1 flex flex-wrap gap-1">
                    {badges
                        .into_iter()
                        .map(|(text, tone)| {
                            let class = match tone {
                                "success" => "rounded-full border border-success/40 bg-success/10 px-2 text-xs text-success",
                                "warning" => "rounded-full border border-warning/40 bg-warning/10 px-2 text-xs text-warning",
                                "info" => "rounded-full border border-info/40 bg-info/10 px-2 text-xs text-info",
                                _ => "rounded-full border border-border bg-surface-2 px-2 text-xs",
                            };
                            view! { <span class=class>{text}</span> }
                        })
                        .collect_view()}
                    {move || {
                        state
                            .get()
                            .locked_reason
                            .map(|r| {
                                let title = r.clone();
                                view! {
                                    <span class="inline-flex items-center gap-1 text-xs text-text-muted" title=title>
                                        <Icon name="lock"/>
                                        <span class="sr-only">{r}</span>
                                    </span>
                                }
                            })
                    }}
                </div>
            </div>
            <div aria-describedby=has_help.then_some(described)>
                {children()}
                {has_help.then(|| view! { <p class="mt-1 text-sm text-text-muted" id=help_id>{help}</p> })}
                <div aria-live="polite">
                    {move || {
                        let s = state.get();
                        let line = |icon: &'static str, class: &'static str, m: String| {
                            view! {
                                <p class=format!("mt-1 flex items-center gap-1 text-sm {class}")>
                                    <Icon name=icon/>
                                    {m}
                                </p>
                            }
                        };
                        s.error
                            .map(|m| line("alert", "text-danger", m))
                            .or(s.disabled_reason.map(|m| line("info", "text-text-muted", m)))
                            .or(s.warning.map(|m| line("warning", "text-warning", m)))
                            .or(s.info.map(|m| line("info", "text-info", m)))
                    }}
                </div>
            </div>
        </div>
    }
}
