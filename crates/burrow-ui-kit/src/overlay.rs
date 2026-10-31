use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::basic::{Tone, button_class};
use crate::icon::Icon;

pub fn focus_first(root: &web_sys::HtmlElement) {
    if let Ok(Some(el)) = root.query_selector(
        "input:not([disabled]), select:not([disabled]), textarea:not([disabled]), button:not([disabled]), a[href]",
    ) && let Ok(el) = el.dyn_into::<web_sys::HtmlElement>()
    {
        let _ = el.focus();
    }
}

pub fn active() -> Option<web_sys::HtmlElement> {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element())
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
}

pub fn trap(e: &web_sys::KeyboardEvent, root: &web_sys::HtmlElement) {
    if e.key() != "Tab" {
        return;
    }
    let Ok(list) = root.query_selector_all(
        "input:not([disabled]), select:not([disabled]), textarea:not([disabled]), button:not([disabled]), a[href]",
    ) else {
        return;
    };
    let n = list.length();
    if n == 0 {
        return;
    }
    let first = list
        .get(0)
        .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok());
    let last = list
        .get(n - 1)
        .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok());
    let now = active();
    if e.shift_key() && now.as_ref() == first.as_ref() {
        e.prevent_default();
        if let Some(l) = last {
            let _ = l.focus();
        }
    } else if !e.shift_key() && now.as_ref() == last.as_ref() {
        e.prevent_default();
        if let Some(f) = first {
            let _ = f.focus();
        }
    }
}

fn tile(tone: Tone) -> &'static str {
    match tone {
        Tone::Danger => "bg-danger-weak text-danger",
        Tone::Warning => "bg-warning-weak text-warning",
        Tone::Success => "bg-success-weak text-success",
        _ => "bg-accent-weak text-accent-ink",
    }
}

#[component]
pub fn Modal(
    #[prop(into)] open: Signal<bool>,
    #[prop(into)] on_close: Callback<()>,
    #[prop(into)] label: Signal<String>,
    class: &'static str,
    #[prop(optional)] testid: &'static str,
    #[prop(default = true)] dismissable: bool,
    #[prop(optional, into)] on_key: Option<Callback<web_sys::KeyboardEvent>>,
    #[prop(optional)] panel: Option<NodeRef<html::Div>>,
    children: ChildrenFn,
) -> impl IntoView {
    let panel = panel.unwrap_or_default();
    let back: StoredValue<Option<web_sys::HtmlElement>, LocalStorage> =
        StoredValue::new_local(None);
    Effect::new(move |was: Option<bool>| {
        let now = open.get();
        if now && was != Some(true) {
            back.set_value(active());
            let enter = move || {
                if let Some(p) = panel.get_untracked() {
                    let inside = active().is_some_and(|a| p.contains(Some(&a)));
                    if !inside {
                        focus_first(&p);
                    }
                }
            };
            if panel.get_untracked().is_some() {
                enter();
            } else {
                request_animation_frame(enter);
            }
        } else if !now && was == Some(true) {
            if let Some(b) = back.get_value() {
                let _ = b.focus();
            }
            back.set_value(None);
        }
        now
    });
    view! {
        <Show when=move || open.get()>
            <div class="fixed inset-0 z-40 bg-scrim" on:click=move |_| if dismissable { on_close.run(()) }></div>
            <div
                node_ref=panel
                class=class
                role="dialog"
                aria-modal="true"
                aria-label=move || label.get()
                data-testid=(!testid.is_empty()).then_some(testid)
                on:keydown=move |e| {
                    if dismissable && e.key() == "Escape" {
                        e.stop_propagation();
                        on_close.run(());
                        return;
                    }
                    if let Some(k) = on_key {
                        k.run(e.clone());
                    }
                    if let Some(p) = panel.get_untracked() {
                        trap(&e, &p);
                    }
                }
            >
                {children()}
            </div>
        </Show>
    }
}

#[component]
pub fn Dialog(
    #[prop(into)] open: RwSignal<bool>,
    #[prop(into)] title: Signal<String>,
    #[prop(optional, into)] sub: Signal<String>,
    #[prop(optional)] icon: &'static str,
    #[prop(optional)] tone: Tone,
    #[prop(optional)] wide: bool,
    #[prop(optional)] testid: &'static str,
    #[prop(default = true)] dismissable: bool,
    #[prop(optional)] foot: Option<ChildrenFn>,
    children: ChildrenFn,
) -> impl IntoView {
    let class = if wide {
        "fixed top-[120px] left-1/2 z-50 max-h-[calc(100vh-160px)] -translate-x-1/2 overflow-y-auto rounded-dialog border border-border-strong bg-surface shadow-overlay dark:bg-surface-2 w-[min(720px,calc(100vw-32px))]"
    } else {
        "fixed top-[120px] left-1/2 z-50 max-h-[calc(100vh-160px)] -translate-x-1/2 overflow-y-auto rounded-dialog border border-border-strong bg-surface shadow-overlay dark:bg-surface-2 w-[min(600px,calc(100vw-32px))]"
    };
    let close = Callback::new(move |_: ()| open.set(false));
    let foot = StoredValue::new(foot);
    let children = StoredValue::new(children);
    view! {
        <Modal open=open on_close=close label=title class=class testid=testid dismissable=dismissable>
            <div class="flex items-start gap-3 px-[22px] pt-5 pb-1.5">
                {(!icon.is_empty()).then(|| view! { <span class=format!("flex size-[34px] shrink-0 items-center justify-center rounded-[9px] {}", tile(tone))><Icon name=icon/></span> })}
                <div class="min-w-0">
                    <h2 class="text-md font-bold">{move || title.get()}</h2>
                    <p class="text-sm text-text-muted">{move || sub.get()}</p>
                </div>
                {dismissable.then(|| view! {
                    <button type="button" class=format!("ml-auto {}", button_class("ghost", true)) aria-label="Close" on:click=move |_| open.set(false)>
                        <Icon name="x"/>
                    </button>
                })}
            </div>
            <div class="px-[22px] pt-2.5 pb-[18px]">{children.with_value(|c| c())}</div>
            {foot.with_value(|f| f.as_ref().map(|f| view! { <div class="flex flex-wrap items-center justify-end gap-2 border-t border-border px-[22px] py-3">{f()}</div> }))}
        </Modal>
    }
}

const TIP: &str = "pointer-events-none absolute z-10 rounded-[6px] bg-tip px-2 py-1 text-xs whitespace-nowrap text-tip-text shadow-overlay";

#[component]
pub fn Tooltip(x: f64, y: f64, #[prop(into)] text: String) -> impl IntoView {
    view! {
        <div class=TIP role="tooltip" data-testid="tooltip" style:left=format!("{x:.0}px") style:top=format!("{y:.0}px")>{text}</div>
    }
}
