use leptos::prelude::*;

use crate::basic::Tone;
use crate::icon::Icon;

pub fn field_class(invalid: bool, mono: bool) -> String {
    format!(
        "h-[38px] w-full min-w-0 rounded-input border bg-input px-2.5 text-[13px] text-text placeholder:text-text-faint outline-none focus:border-accent focus:ring-3 focus:ring-accent-weak disabled:opacity-55 disabled:cursor-not-allowed {}{}",
        if invalid {
            "border-danger ring-3 ring-danger-weak"
        } else {
            "border-border-strong"
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
    #[prop(optional, into)] prefix: Option<String>,
    #[prop(optional, into)] suffix: Option<String>,
) -> impl IntoView {
    let suffix = suffix.filter(|x| !x.is_empty());
    let pad = format!(
        "{}{}",
        if prefix.is_some() { " pl-6" } else { "" },
        if suffix.is_some() { " pr-24" } else { "" }
    );
    view! {
        <div class="relative min-w-0 flex-1">
            {prefix.map(|p| view! { <span class="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 font-mono text-[13px] font-bold text-text-faint">{p}</span> })}
            <input
                id=id
                type="text"
                class=move || format!("{}{}{}", field_class(invalid.get(), mono), pad, if numeric { " tabular-nums" } else { "" })
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
            {suffix.map(|s| view! { <span class="pointer-events-none absolute top-1/2 right-2.5 -translate-y-1/2 text-xs text-text-muted">{s}</span> })}
        </div>
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
            class="w-full rounded-input border border-border-strong bg-input p-2.5 font-mono text-[13px] text-text outline-none focus:border-accent focus:ring-3 focus:ring-accent-weak"
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
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional)] small: bool,
) -> impl IntoView {
    view! {
        <div class="relative min-w-0">
            <select
                id=id
                class=move || {
                    let base = format!("{} appearance-none pr-8", field_class(invalid.get(), false));
                    if small { base.replace("h-[38px]", "h-7").replace("text-[13px]", "text-xs") } else { base }
                }
                aria-label=label
                aria-invalid=move || invalid.get().then_some("true")
                disabled=move || disabled.get()
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
            <span class="pointer-events-none absolute top-1/2 right-2.5 -translate-y-1/2 text-text-muted"><Icon name="chevron-down" small=true/></span>
        </div>
    }
}

#[component]
pub fn Toggle(
    #[prop(into)] id: String,
    #[prop(into)] checked: Signal<bool>,
    #[prop(into)] on_change: Callback<bool>,
    #[prop(into)] label: Signal<String>,
    #[prop(optional, into)] danger: bool,
    #[prop(optional, into)] disabled: Signal<bool>,
) -> impl IntoView {
    let _ = danger;
    view! {
        <label class="inline-flex cursor-pointer items-center gap-2 text-sm has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-55">
            <input
                id=id
                type="checkbox"
                role="switch"
                class="peer sr-only"
                prop:checked=move || checked.get()
                aria-checked=move || checked.get().to_string()
                disabled=move || disabled.get()
                on:change:target=move |e| on_change.run(e.target().checked())
            />
            <span
                class="relative h-5 w-[34px] shrink-0 rounded-[10px] bg-border-strong transition-colors peer-checked:bg-accent peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-accent after:absolute after:top-[3px] after:left-[3px] after:size-3.5 after:rounded-full after:bg-white after:transition-transform peer-checked:after:translate-x-[14px] dark:after:bg-[#C9CED6] dark:peer-checked:after:bg-on-accent"
                aria-hidden="true"
            ></span>
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
    #[prop(optional, into)] disabled: Signal<bool>,
) -> impl IntoView {
    view! {
        <div
            class="inline-flex max-w-full flex-wrap gap-0.5 rounded-md border border-border bg-surface-3 p-0.5 dark:bg-input"
            role="radiogroup"
            aria-label=label
        >
            {options
                .into_iter()
                .map(|(v, l, danger)| {
                    let v2 = v.clone();
                    let v3 = v.clone();
                    let name = name.clone();
                    view! {
                        <label class=move || {
                            let on = value.get() == v2;
                            let base = "flex h-8 cursor-pointer items-center rounded-sm px-[11px] text-sm font-semibold has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-accent";
                            match (on, danger) {
                                (true, true) => format!("{base} bg-danger-weak text-danger ring-1 ring-danger/45 ring-inset"),
                                (true, false) => format!("{base} bg-surface text-text shadow-sm ring-1 ring-border ring-inset dark:bg-surface-3 dark:ring-border-strong"),
                                (false, _) => format!("{base} text-text-muted hover:text-text"),
                            }
                        }>
                            <input
                                type="radio"
                                class="sr-only"
                                name=name
                                value=v.clone()
                                disabled=move || disabled.get()
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
    #[prop(optional, into)] meta: Option<Callback<String, String>>,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] empty_label: String,
    #[prop(optional, into)] invalid: Signal<bool>,
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
    view! {
        <div
            class=move || format!(
                "flex min-h-10 flex-wrap items-center gap-[5px] rounded-input border bg-input p-1 focus-within:border-accent focus-within:ring-3 focus-within:ring-accent-weak {}",
                if invalid.get() { "border-danger" } else { "border-border-strong" }
            )
            data-chips=""
        >
            {move || {
                items
                    .get()
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let problem = check.run(item.clone());
                        let tone = if problem.is_some() { Tone::Danger } else { Tone::Neutral };
                        let title = problem.clone().unwrap_or_default();
                        let label = format!("Remove {item}");
                        let note = meta.map(|m| m.run(item.clone())).filter(|m| !m.is_empty());
                        let chip = match tone {
                            Tone::Danger => "bg-danger-weak text-danger ring-1 ring-danger/40",
                            _ => "bg-surface-3 text-text ring-1 ring-border",
                        };
                        view! {
                            <span
                                class=format!("inline-flex h-[26px] items-center gap-1.5 rounded-sm pr-1 pl-2 font-mono text-xs font-semibold {chip}")
                                title=title
                                data-chip=item.clone()
                                data-invalid=problem.is_some().then_some("true")
                            >
                                {item.clone()}
                                {note.map(|n| view! { <span class="font-sans text-[11px] font-normal text-text-muted">{n}</span> })}
                                <button
                                    type="button"
                                    class="flex size-[18px] cursor-pointer items-center justify-center rounded text-text-faint hover:text-danger"
                                    aria-label=label
                                    on:click=move |_| {
                                        let mut l = items.get_untracked();
                                        if i < l.len() {
                                            l.remove(i);
                                            on_change.run(l);
                                        }
                                    }
                                >
                                    <Icon name="x" small=true/>
                                </button>
                            </span>
                        }
                    })
                    .collect_view()
            }}
            <input
                id=id
                type="text"
                class="h-7 min-w-36 flex-1 bg-transparent px-1.5 font-mono text-xs outline-none placeholder:font-sans placeholder:text-text-faint"
                placeholder=move || if items.with(Vec::is_empty) && !empty_label.is_empty() { empty_label.clone() } else { placeholder.clone() }
                autocomplete="off"
                spellcheck="false"
                prop:value=move || typing.get()
                on:input:target=move |e| {
                    let v = e.target().value();
                    let split = v.contains([',', ';', '\n', ' ']);
                    typing.set(v);
                    if split {
                        commit();
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
        </div>
    }
}

#[component]
pub fn ConfirmInput(
    #[prop(into)] token: String,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_input: Callback<String>,
    #[prop(optional, into)] message: String,
) -> impl IntoView {
    let id = format!(
        "confirm-{}",
        token.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
    );
    let label = format!("Type {token} to confirm");
    let t = token.clone();
    view! {
        <div class="rounded-[10px] border border-danger/45 bg-danger-weak p-3.5 text-[13px] text-co-danger">
            {(!message.is_empty()).then(|| view! { <p class="mb-1 flex items-center gap-2 font-semibold"><Icon name="alert"/>{message}</p> })}
            <label class="mb-2 block text-text" for=id.clone()>
                "Type "
                <strong class="font-mono">{token.clone()}</strong>
                " to apply."
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
