use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::icon::Icon;

use super::toggle;

#[derive(Clone, Copy)]
pub(super) struct Create {
    pub(super) creating: RwSignal<bool>,
    pub(super) new_name: RwSignal<String>,
    pub(super) create_error: RwSignal<Option<String>>,
}

pub(super) fn create_section(
    cb: Callback<String, Option<String>>,
    label: String,
    id: &str,
    c: Create,
    selected: Signal<Vec<String>>,
    on_change: Callback<Vec<String>>,
    multi: bool,
) -> impl IntoView + use<> {
    let Create {
        creating,
        new_name,
        create_error,
    } = c;
    let combo_id = id.to_string();
    let name_id = format!("{id}-new-name");
    let refocus = {
        let combo_id = combo_id.clone();
        move || {
            if let Some(el) = document().get_element_by_id(&combo_id)
                && let Ok(el) = el.dyn_into::<web_sys::HtmlElement>()
            {
                let _ = el.focus();
            }
        }
    };
    let create = {
        let refocus = refocus.clone();
        move || {
            let n = new_name.get_untracked().trim().to_string();
            match cb.run(n.clone()) {
                None => {
                    refocus();
                    creating.set(false);
                    new_name.set(String::new());
                    on_change.run(toggle(&selected.get_untracked(), &n, multi));
                }
                Some(err) => create_error.set(Some(err)),
            }
        }
    };
    let placeholder = format!("{} name", label.trim_end_matches('…'));
    view! {
        <div class="mt-1 border-t border-border pt-1">
            {move || if creating.get() {
                let (create, create2, refocus) = (create.clone(), create.clone(), refocus.clone());
                view! {
                    <div class="px-2.5 py-1.5">
                        <div class="flex items-center gap-2">
                            <input
                                id=name_id.clone()
                                type="text"
                                class="h-8 min-w-0 flex-1 rounded-input border border-border-strong bg-input px-2 font-mono text-sm outline-none placeholder:font-sans placeholder:text-text-faint focus:border-accent"
                                aria-label=placeholder.clone()
                                placeholder=placeholder.clone()
                                data-testid="combo-new-name"
                                prop:value=move || new_name.get()
                                on:input:target=move |e| { new_name.set(e.target().value()); create_error.set(None); }
                                on:keydown=move |e| {
                                    if e.key() == "Enter" {
                                        e.prevent_default();
                                        e.stop_propagation();
                                        create();
                                    }
                                    if e.key() == "Escape" {
                                        e.stop_propagation();
                                        refocus();
                                        creating.set(false);
                                    }
                                }
                            />
                            <button
                                type="button"
                                class="h-8 cursor-pointer rounded-input bg-accent px-3 text-sm font-semibold text-on-accent hover:brightness-95 disabled:cursor-not-allowed disabled:opacity-55"
                                data-testid="combo-create"
                                disabled=move || new_name.with(|n| n.trim().is_empty())
                                on:mousedown=move |e| e.prevent_default()
                                on:click=move |_| create2()
                            >
                                "Create"
                            </button>
                        </div>
                        <p class="mt-1 text-xs text-danger" data-testid="combo-new-error">{move || create_error.get()}</p>
                    </div>
                }
                .into_any()
            } else {
                let label = label.clone();
                let (refocus, name_id) = (refocus.clone(), name_id.clone());
                view! {
                    <button
                        type="button"
                        class="flex h-[34px] w-full cursor-pointer items-center gap-2 rounded-input px-2.5 text-sm font-semibold text-accent-ink hover:bg-surface-3"
                        data-testid="combo-new"
                        on:mousedown=move |e| e.prevent_default()
                        on:click=move |_| {
                            refocus();
                            creating.set(true);
                            let name_id = name_id.clone();
                            request_animation_frame(move || {
                                if let Some(el) = document().get_element_by_id(&name_id)
                                    && let Ok(el) = el.dyn_into::<web_sys::HtmlElement>()
                                {
                                    let _ = el.focus();
                                }
                            });
                        }
                    >
                        <Icon name="plus" small=true/>
                        {label}
                    </button>
                }
                .into_any()
            }}
        </div>
    }
}
