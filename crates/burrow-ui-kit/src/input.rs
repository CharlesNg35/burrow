use leptos::prelude::*;

use crate::basic::{Tone, button_class};
use crate::icon::Icon;

const FIELD: &str = "h-9 w-full rounded-input border bg-surface px-2.5 text-base text-text placeholder:text-text-muted disabled:opacity-60";

fn field_class(invalid: bool, mono: bool) -> String {
    format!(
        "{FIELD} {}{}",
        if invalid {
            "border-danger"
        } else {
            "border-border"
        },
        if mono { " font-mono" } else { "" }
    )
}

#[component]
pub fn TextInput(
    #[prop(into)] id: String,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_input: Callback<String>,
    #[prop(optional, into)] on_blur: Option<Callback<()>>,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] invalid: Signal<bool>,
    #[prop(optional, into)] described_by: Signal<String>,
    #[prop(optional)] numeric: bool,
    #[prop(optional)] mono: bool,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] label: Option<String>,
) -> impl IntoView {
    view! {
        <input
            id=id
            type="text"
            class=move || field_class(invalid.get(), mono)
            inputmode=numeric.then_some("numeric")
            autocomplete="off"
            spellcheck="false"
            placeholder=placeholder
            aria-label=label
            aria-invalid=move || invalid.get().then_some("true")
            aria-describedby=move || Some(described_by.get()).filter(|d| !d.is_empty())
            aria-errormessage=move || {
                Some(described_by.get()).filter(|d| !d.is_empty() && invalid.get())
            }
            disabled=move || disabled.get()
            prop:value=move || value.get()
            on:input:target=move |e| on_input.run(e.target().value())
            on:blur=move |_| {
                if let Some(b) = on_blur {
                    let _ = b.try_run(());
                }
            }
        />
    }
}

#[component]
pub fn TextArea(
    #[prop(into)] id: String,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_input: Callback<String>,
    #[prop(optional)] rows: u32,
) -> impl IntoView {
    view! {
        <textarea
            id=id
            rows=rows.max(3)
            spellcheck="false"
            class="w-full rounded-input border border-border bg-surface p-2.5 font-mono text-sm text-text"
            prop:value=move || value.get()
            on:input:target=move |e| on_input.run(e.target().value())
        ></textarea>
    }
}

#[component]
pub fn Select(
    #[prop(into)] id: String,
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_change: Callback<String>,
    #[prop(optional, into)] label: Option<String>,
    #[prop(optional, into)] invalid: Signal<bool>,
) -> impl IntoView {
    view! {
        <select
            id=id
            class=move || field_class(invalid.get(), false)
            aria-label=label
            aria-invalid=move || invalid.get().then_some("true")
            on:change:target=move |e| on_change.run(e.target().value())
        >
            {move || {
                let current = value.get();
                options
                    .get()
                    .into_iter()
                    .map(|(v, l)| {
                        let selected = v == current;
                        view! { <option value=v selected=selected>{l}</option> }
                    })
                    .collect_view()
            }}
        </select>
    }
}

#[component]
pub fn Toggle(
    #[prop(into)] id: String,
    #[prop(into)] checked: Signal<bool>,
    #[prop(into)] on_change: Callback<bool>,
    #[prop(into)] label: Signal<String>,
    #[prop(optional, into)] danger: bool,
) -> impl IntoView {
    view! {
        <label class="inline-flex cursor-pointer items-center gap-2">
            <input
                id=id
                type="checkbox"
                role="switch"
                class=if danger { "size-4 accent-danger" } else { "size-4 accent-accent" }
                prop:checked=move || checked.get()
                aria-checked=move || checked.get().to_string()
                on:change:target=move |e| on_change.run(e.target().checked())
            />
            <span>{move || label.get()}</span>
        </label>
    }
}

#[component]
pub fn Segmented(
    #[prop(into)] name: String,
    #[prop(into)] options: Vec<(String, String, bool)>,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_change: Callback<String>,
    #[prop(optional, into)] label: Option<String>,
) -> impl IntoView {
    view! {
        <div class="inline-flex flex-wrap overflow-hidden rounded-input border border-border" role="radiogroup" aria-label=label>
            {options
                .into_iter()
                .map(|(v, l, danger)| {
                    let v2 = v.clone();
                    let v3 = v.clone();
                    let name = name.clone();
                    view! {
                        <label class=move || {
                            let on = value.get() == v2;
                            match (on, danger) {
                                (true, true) => "cursor-pointer border-r border-border bg-danger px-3 py-1.5 text-sm text-white last:border-r-0",
                                (true, false) => "cursor-pointer border-r border-border bg-accent px-3 py-1.5 text-sm text-white last:border-r-0",
                                (false, _) => "cursor-pointer border-r border-border bg-surface px-3 py-1.5 text-sm hover:bg-surface-2 last:border-r-0",
                            }
                        }>
                            <input
                                type="radio"
                                class="sr-only"
                                name=name
                                value=v.clone()
                                prop:checked=move || value.get() == v3
                                on:change=move |_| on_change.run(v.clone())
                            />
                            {l}
                        </label>
                    }
                })
                .collect_view()}
        </div>
    }
}

pub fn split_entries(text: &str) -> Vec<String> {
    text.split([',', ';', '\n', ' ', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[component]
pub fn ChipInput(
    #[prop(into)] id: String,
    #[prop(into)] items: Signal<Vec<String>>,
    #[prop(into)] on_change: Callback<Vec<String>>,
    #[prop(into)] check: Callback<String, Option<String>>,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] empty_label: String,
    #[prop(default = None)] extra: Option<(String, String)>,
) -> impl IntoView {
    let typing = RwSignal::new(String::new());
    let commit = move || {
        let text = typing.get_untracked();
        let mut list = items.get_untracked();
        let mut added = false;
        for e in split_entries(&text) {
            if !list.contains(&e) {
                list.push(e);
                added = true;
            }
        }
        typing.set(String::new());
        if added {
            on_change.run(list);
        }
    };
    let extra_button = extra.map(|(label, value)| {
        view! {
            <button
                type="button"
                class=button_class("ghost", true)
                on:click=move |_| {
                    let mut list = items.get_untracked();
                    if !list.contains(&value) {
                        list.push(value.clone());
                        on_change.run(list);
                    }
                }
            >
                {label}
            </button>
        }
    });
    view! {
        <div class="flex flex-wrap items-center gap-1.5 rounded-input border border-border bg-surface p-1.5" data-chips="">
            {move || {
                let list = items.get();
                if list.is_empty() && !empty_label.is_empty() {
                    return view! { <span class="px-1 text-sm text-text-muted">{empty_label.clone()}</span> }.into_any();
                }
                list.iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let problem = check.run(item.clone());
                        let tone = if problem.is_some() { Tone::Danger } else { Tone::Neutral };
                        let title = problem.clone().unwrap_or_default();
                        let label = format!("Remove {item}");
                        view! {
                            <span
                                class=format!("inline-flex items-center gap-1 rounded-input border px-2 py-0.5 font-mono text-sm {}", tone.soft())
                                title=title
                                data-chip=item.clone()
                                data-invalid=problem.is_some().then_some("true")
                            >
                                {item.clone()}
                                <button
                                    type="button"
                                    class="cursor-pointer text-text-muted hover:text-danger"
                                    aria-label=label
                                    on:click=move |_| {
                                        let mut l = items.get_untracked();
                                        if i < l.len() {
                                            l.remove(i);
                                            on_change.run(l);
                                        }
                                    }
                                >
                                    <Icon name="close"/>
                                </button>
                            </span>
                        }
                        .into_any()
                    })
                    .collect_view()
                    .into_any()
            }}
            <input
                id=id
                type="text"
                class="min-w-32 flex-1 bg-transparent px-1 py-0.5 font-mono text-sm outline-none"
                placeholder=placeholder
                autocomplete="off"
                spellcheck="false"
                prop:value=move || typing.get()
                on:input:target=move |e| {
                    let v = e.target().value();
                    if v.contains([',', ';', '\n', ' ']) {
                        typing.set(v);
                        commit();
                    } else {
                        typing.set(v);
                    }
                }
                on:keydown=move |e| {
                    if e.key() == "Enter" {
                        e.prevent_default();
                        commit();
                    } else if e.key() == "Backspace" && typing.get_untracked().is_empty() {
                        let mut l = items.get_untracked();
                        if l.pop().is_some() {
                            on_change.run(l);
                        }
                    }
                }
                on:blur=move |_| {
                    if typing.try_get_untracked().is_some() {
                        commit();
                    }
                }
            />
            {extra_button}
        </div>
    }
}

#[component]
pub fn ConfirmInput(
    #[prop(into)] token: String,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_input: Callback<String>,
) -> impl IntoView {
    let id = format!(
        "confirm-{}",
        token.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
    );
    let label = format!("Type {token} to confirm");
    let t = token.clone();
    view! {
        <div class="mt-2">
            <label class="text-sm" for=id.clone()>
                "Type "
                <strong class="font-mono">{token.clone()}</strong>
                " to confirm"
            </label>
            <input
                id=id
                type="text"
                autocomplete="off"
                spellcheck="false"
                class=move || field_class(!value.get().is_empty() && value.get().trim() != t, true)
                aria-label=label
                data-confirm=token.clone()
                prop:value=move || value.get()
                on:input:target=move |e| on_input.run(e.target().value())
            />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_lists_split_into_entries() {
        assert_eq!(
            split_entries("10.0.0.0/8, 192.168.1.0/24\n 127.0.0.1;;"),
            vec!["10.0.0.0/8", "192.168.1.0/24", "127.0.0.1"]
        );
        assert!(split_entries(" , ").is_empty());
    }
}
