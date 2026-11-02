use leptos::prelude::*;

use crate::basic::{Kbd, LINK, Tone};
use crate::icon::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonLook {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Danger,
    DangerPending,
    GhostDanger,
    Tinted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    #[default]
    Normal,
    Small,
}

pub fn button_class(look: ButtonLook, size: ButtonSize) -> String {
    let look = match look {
        ButtonLook::Primary => {
            "border-accent bg-accent text-on-accent hover:brightness-95 dark:hover:brightness-110 disabled:border-border disabled:bg-surface-3 disabled:text-text-faint disabled:opacity-100 disabled:hover:brightness-100"
        }
        ButtonLook::Danger => {
            "border-danger bg-danger text-danger-ink hover:brightness-95 dark:hover:brightness-110"
        }
        ButtonLook::DangerPending => "border-danger/45 bg-transparent text-danger",
        ButtonLook::Tinted => {
            "border-accent-line bg-accent-weak text-accent-ink hover:brightness-105"
        }
        ButtonLook::Ghost => {
            "border-transparent bg-transparent text-text-muted hover:bg-surface-3 hover:text-text"
        }
        ButtonLook::GhostDanger => {
            "border-transparent bg-transparent text-danger hover:bg-danger-weak"
        }
        ButtonLook::Secondary => {
            "border-border-strong bg-surface text-text hover:bg-surface-3 dark:bg-surface-2"
        }
    };
    let size = match size {
        ButtonSize::Small => "h-[26px] px-[9px] text-xs",
        ButtonSize::Normal => "h-9 px-3 text-sm",
    };
    format!(
        "inline-flex items-center justify-center gap-1.5 rounded-input border font-semibold whitespace-nowrap cursor-pointer no-underline transition-colors disabled:cursor-not-allowed disabled:opacity-55 {look} {size}"
    )
}

#[component]
pub fn Button(
    #[prop(into)] label: Signal<String>,
    #[prop(optional)] variant: ButtonLook,
    #[prop(optional)] size: ButtonSize,
    #[prop(optional)] icon: &'static str,
    #[prop(optional)] kbd: &'static str,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] reason: Signal<String>,
    #[prop(optional, into)] busy: Signal<bool>,
    #[prop(optional)] testid: &'static str,
    #[prop(into)] on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <span class="inline-flex" title=move || Some(reason.get()).filter(|r| !r.is_empty())>
        <button
            type="button"
            class=move || button_class(if variant == ButtonLook::Danger && disabled.get() { ButtonLook::DangerPending } else { variant }, size)
            disabled=move || disabled.get() || busy.get()
            aria-busy=move || busy.get().then_some("true")
            data-testid=(!testid.is_empty()).then_some(testid)
            on:click=move |_| on_click.run(())
        >
            {move || busy.get().then(|| view! { <SpinDot/> })}
            {(!icon.is_empty()).then(|| view! { <Icon name=icon/> })}
            <span>{move || label.get()}</span>
            {(!kbd.is_empty()).then(|| view! { <Kbd text=kbd/> })}
        </button>
        </span>
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
pub fn IconButton(
    icon: &'static str,
    #[prop(into)] label: String,
    #[prop(optional)] testid: &'static str,
    #[prop(into)] on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="flex size-7 shrink-0 cursor-pointer items-center justify-center rounded-md text-text-muted hover:bg-surface-3 hover:text-text"
            aria-label=label.clone()
            title=label
            data-testid=(!testid.is_empty()).then_some(testid)
            on:click=move |_| on_click.run(())
        >
            <Icon name=icon small=true/>
        </button>
    }
}

pub const MENU_ITEM: &str =
    "block w-full cursor-pointer rounded-md px-2 py-1.5 text-left text-sm hover:bg-surface-3";

pub fn menu_class(place: &str, roomy: bool) -> String {
    let inside = if roomy { "p-3 text-sm" } else { "p-1" };
    format!(
        "absolute z-40 rounded-[10px] border border-border-strong bg-surface text-text shadow-overlay outline-none dark:bg-surface-2 {inside} {place}"
    )
}

#[component]
pub fn LinkButton(
    #[prop(optional)] testid: &'static str,
    #[prop(optional)] class: &'static str,
    #[prop(into)] on_click: Callback<()>,
    children: Children,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class=format!("cursor-pointer font-semibold {LINK} {class}")
            data-testid=(!testid.is_empty()).then_some(testid)
            on:click=move |_| on_click.run(())
        >
            {children()}
        </button>
    }
}

#[component]
pub fn MenuItem(
    #[prop(into)] label: String,
    #[prop(optional)] tone: Tone,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] reason: Signal<String>,
    #[prop(optional)] testid: &'static str,
    #[prop(into)] on_click: Callback<()>,
) -> impl IntoView {
    let ink = match tone {
        Tone::Danger => " text-danger",
        _ => "",
    };
    view! {
        <button
            type="button"
            class=format!("{MENU_ITEM} disabled:cursor-not-allowed disabled:opacity-55 disabled:hover:bg-transparent{ink}")
            data-testid=(!testid.is_empty()).then_some(testid)
            disabled=move || disabled.get()
            title=move || Some(reason.get()).filter(|r| !r.is_empty())
            on:click=move |_| on_click.run(())
        >
            {label}
        </button>
    }
}
