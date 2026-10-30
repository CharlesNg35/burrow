mod create;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::basic::Tone;
use crate::icon::Icon;

use create::{Create, create_section};

#[derive(Debug, Clone, PartialEq)]
pub struct ComboOption {
    pub value: String,
    pub label: String,
    pub facet: String,
    pub dot: Option<Tone>,
}

pub fn filter_options(options: &[ComboOption], query: &str) -> Vec<ComboOption> {
    let q = query.trim().to_lowercase();
    options
        .iter()
        .filter(|o| {
            q.is_empty()
                || o.label.to_lowercase().contains(&q)
                || o.value.to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

pub fn toggle(selected: &[String], value: &str, multi: bool) -> Vec<String> {
    if !multi {
        return vec![value.to_string()];
    }
    let mut out = selected.to_vec();
    match out.iter().position(|x| x == value) {
        Some(i) => {
            out.remove(i);
        }
        None => out.push(value.to_string()),
    }
    out
}

fn dot_class(tone: Option<Tone>) -> &'static str {
    match tone {
        Some(Tone::Success) => "bg-success",
        Some(Tone::Warning) => "bg-warning",
        Some(Tone::Danger) => "bg-danger",
        _ => "bg-transparent ring-[1.5px] ring-text-faint ring-inset",
    }
}

const POPOVER_ROOM: f64 = 240.0;

#[component]
pub fn Combobox(
    #[prop(into)] id: String,
    #[prop(into)] options: Signal<Vec<ComboOption>>,
    #[prop(into)] selected: Signal<Vec<String>>,
    #[prop(into)] on_change: Callback<Vec<String>>,
    #[prop(optional)] multi: bool,
    #[prop(optional)] clearable: bool,
    #[prop(optional, into)] header: String,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] label: String,
    #[prop(optional, into)] invalid: Signal<bool>,
    #[prop(optional, into)] described_by: String,
    #[prop(optional, into)] create_label: String,
    #[prop(default = None)] on_create: Option<Callback<String, Option<String>>>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let creating = RwSignal::new(false);
    let new_name = RwSignal::new(String::new());
    let create_error: RwSignal<Option<String>> = RwSignal::new(None);
    let query = RwSignal::new(String::new());
    let active = RwSignal::new(0usize);
    let list_id = format!("{id}-list");
    let visible = Memo::new(move |_| options.with(|o| filter_options(o, &query.get())));
    let pick = move |value: String| {
        let next = toggle(&selected.get_untracked(), &value, multi);
        on_change.run(next);
        query.set(String::new());
        if !multi {
            open.set(false);
        }
    };
    let remove = move |value: String| remove_value(selected, on_change, value);
    let chips = chips(options, selected, multi, clearable, remove);
    let rows = option_rows(id.clone(), selected, visible, active, pick, multi);
    let input_id = id.clone();
    let up = RwSignal::new(false);
    place_popover(id.clone(), open, up);
    Effect::new(move |_| {
        if !open.get() && creating.get_untracked() {
            creating.set(false);
            new_name.set(String::new());
            create_error.set(None);
        }
    });
    let c = Create {
        creating,
        new_name,
        create_error,
    };
    let lid = list_id.clone();
    let act_id = {
        let id = id.clone();
        move || open.get().then(|| format!("{id}-opt-{}", active.get()))
    };
    view! {
        <div
            class="relative"
            on:focusout=move |e| {
                let inside = e
                    .related_target()
                    .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                    .zip(e.current_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()))
                    .is_some_and(|(t, me)| me.contains(Some(&t)));
                if !inside {
                    open.set(false);
                }
            }
        >
            <div
                class=move || format!(
                    "flex min-h-10 flex-wrap items-center gap-[5px] rounded-input border bg-input p-1 {}",
                    if invalid.get() { "border-danger ring-3 ring-danger-weak" } else { "border-border-strong focus-within:border-accent focus-within:ring-3 focus-within:ring-accent-weak" }
                )
                on:click=move |_| {
                    if let Some(el) = document().get_element_by_id(&input_id)
                        && let Ok(el) = el.dyn_into::<web_sys::HtmlElement>()
                    {
                        let _ = el.focus();
                    }
                    open.set(true);
                }
            >
                {chips}
                <input
                    id=id.clone()
                    type="text"
                    role="combobox"
                    autocomplete="off"
                    spellcheck="false"
                    class="h-[26px] min-w-24 flex-1 bg-transparent px-1.5 text-sm outline-none placeholder:text-text-faint"
                    placeholder=move || if multi || selected.with(|s| s.iter().all(String::is_empty)) { placeholder.clone() } else { String::new() }
                    aria-label=(!label.is_empty()).then_some(label)
                    aria-expanded=move || open.get().to_string()
                    aria-controls=lid.clone()
                    aria-autocomplete="list"
                    aria-activedescendant=act_id
                    aria-invalid=move || invalid.get().then_some("true")
                    aria-describedby=(!described_by.is_empty()).then_some(described_by)
                    prop:value=move || query.get()
                    on:focus=move |_| open.set(true)
                    on:input:target=move |e| {
                        query.set(e.target().value());
                        active.set(0);
                        open.set(true);
                    }
                    on:keydown=move |e| match e.key().as_str() {
                        "ArrowDown" => {
                            e.prevent_default();
                            open.set(true);
                            let n = visible.with(Vec::len);
                            if n > 0 {
                                active.update(|a| *a = (*a + 1).min(n - 1));
                            }
                        }
                        "ArrowUp" => {
                            e.prevent_default();
                            active.update(|a| *a = a.saturating_sub(1));
                        }
                        "Enter" => {
                            e.prevent_default();
                            if let Some(o) = visible.with(|v| v.get(active.get_untracked()).cloned()) {
                                pick(o.value);
                            }
                        }
                        "Escape" => {
                            if open.get_untracked() {
                                e.stop_propagation();
                                open.set(false);
                            }
                        }
                        "Backspace" if multi && query.with_untracked(String::is_empty) => {
                            if let Some(last) = selected.get_untracked().last().cloned() {
                                remove(last);
                            }
                        }
                        _ => {}
                    }
                />
                {(!multi).then(|| view! { <span class="pointer-events-none mr-1.5 ml-auto text-text-muted" aria-hidden="true" data-single=""><Icon name="chevron-down" small=true/></span> })}
            </div>
            <Show when=move || open.get()>
                <div class=move || format!("absolute left-0 z-30 w-[min(470px,100%)] min-w-64 rounded-[10px] border border-border-strong bg-surface p-[5px] shadow-overlay dark:bg-surface-2 {}", if up.get() { "bottom-full mb-1" } else { "top-full mt-1" })>
                    {(!header.is_empty()).then(|| view! { <div class="px-2.5 pt-1.5 pb-1 text-2xs font-bold tracking-[0.08em] text-text-faint uppercase">{header.clone()}</div> })}
                    <ul id=list_id.clone() role="listbox" aria-multiselectable=multi.then_some("true") class="max-h-72 overflow-y-auto">
                        {rows.clone()}
                    </ul>
                    {on_create.map(|cb| create_section(cb, create_label.clone(), &id, c, selected, on_change, multi))}
                </div>
            </Show>
        </div>
    }
}

fn remove_value(selected: Signal<Vec<String>>, on_change: Callback<Vec<String>>, value: String) {
    let next: Vec<String> = selected
        .get_untracked()
        .into_iter()
        .filter(|x| *x != value)
        .collect();
    on_change.run(next);
}

fn chips(
    options: Signal<Vec<ComboOption>>,
    selected: Signal<Vec<String>>,
    multi: bool,
    clearable: bool,
    remove: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl Fn() -> AnyView + Send + Sync + 'static {
    let label_of = move |v: &str| {
        options.with(|o| {
            o.iter()
                .find(|x| x.value == v)
                .map(|x| (x.label.clone(), x.dot))
        })
    };
    move || {
        selected
        .get()
        .into_iter()
        .filter(|v| multi || clearable || !v.is_empty())
        .map(|v| {
            let (text, dot, missing) = match label_of(&v) {
                Some((l, d)) => (l, d, false),
                None => (format!("{v} (removed)"), Some(Tone::Danger), true),
            };
            let v2 = v.clone();
            let removable = multi || clearable;
            let look = match (missing, multi) {
                (true, _) => "bg-danger-weak text-danger",
                (false, true) => "bg-surface-3 ring-1 ring-border ring-inset dark:ring-0",
                (false, false) => "",
            };
            view! {
                <span class=format!("inline-flex h-[26px] items-center gap-1.5 rounded-sm pr-1 pl-2 text-sm font-semibold {look}") data-chip=v.clone()>
                    <span class=format!("size-2 rounded-full {}", dot_class(dot)) aria-hidden="true"></span>
                    {text.clone()}
                    {removable.then(|| view! {
                        <button
                            type="button"
                            class="flex size-[18px] cursor-pointer items-center justify-center rounded-xs text-text-faint hover:bg-border hover:text-text"
                            aria-label=format!("Remove {text}")
                            data-remove=v2.clone()
                            on:mousedown=move |e| e.prevent_default()
                            on:click=move |_| remove(v2.clone())
                        >
                            <Icon name="x" small=true/>
                        </button>
                    })}
                </span>
            }
        })
        .collect_view()
            .into_any()
    }
}

fn option_rows(
    id: String,
    selected: Signal<Vec<String>>,
    visible: Memo<Vec<ComboOption>>,
    active: RwSignal<usize>,
    pick: impl Fn(String) + Copy + Send + Sync + 'static,
    multi: bool,
) -> impl Fn() -> AnyView + Clone + Send + Sync + 'static {
    move || {
        let sel = selected.get();
        let list = visible.get();
        if list.is_empty() {
            return view! { <li class="px-2.5 py-2 text-sm text-text-muted" role="presentation">"Nothing matches"</li> }.into_any();
        }
        let act = active.get().min(list.len() - 1);
        list.into_iter()
            .enumerate()
            .map(|(i, o)| {
                let on = sel.contains(&o.value);
                let value = o.value.clone();
                view! {
                    <li
                        id=format!("{id}-opt-{i}")
                        role="option"
                        aria-selected=on.to_string()
                        class="flex h-[34px] cursor-pointer items-center gap-2.5 rounded-input px-2.5 text-sm"
                        class:bg-surface-3=i == act
                        data-option=o.value.clone()
                        on:mousedown=move |e| e.prevent_default()
                        on:mouseenter=move |_| active.set(i)
                        on:click=move |e| {
                            let on_input = e
                                .target()
                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                .is_some();
                            if !on_input {
                                pick(value.clone());
                            }
                        }
                    >
                        {multi.then(|| {
                            let value = o.value.clone();
                            view! {
                                <input
                                    type="checkbox"
                                    tabindex="-1"
                                    class="size-[15px] accent-accent"
                                    aria-hidden="true"
                                    prop:checked=on
                                    on:change=move |_| pick(value.clone())
                                />
                            }
                        })}
                        <span class=format!("size-2 shrink-0 rounded-full {}", dot_class(o.dot)) aria-hidden="true"></span>
                        <span class="font-semibold">{o.label.clone()}</span>
                        <span class="ml-auto truncate text-[11.5px] text-text-muted">{o.facet.clone()}</span>
                        {(!multi && on).then(|| view! { <span class="text-accent-ink"><Icon name="check" small=true/></span> })}
                    </li>
                }
            })
            .collect_view()
            .into_any()
    }
}

fn place_popover(id: String, open: RwSignal<bool>, up: RwSignal<bool>) {
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let Some(field) = document().get_element_by_id(&id) else {
            return;
        };
        let h = window()
            .inner_height()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let floor = document()
            .query_selector("[data-sticky-bar]")
            .ok()
            .flatten()
            .map(|b| b.get_bounding_client_rect().top())
            .filter(|t| *t > 0.0 && *t < h)
            .unwrap_or(h);
        let r = field.get_bounding_client_rect();
        let below = floor - r.bottom();
        let above = r.top() - 72.0;
        up.set(below < POPOVER_ROOM && above > below);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(v: &str) -> ComboOption {
        ComboOption {
            value: v.into(),
            label: v.into(),
            facet: String::new(),
            dot: None,
        }
    }

    #[test]
    fn typing_filters_and_picking_toggles() {
        let all = vec![opt("lab-k8s-1"), opt("lab-k8s-2"), opt("office-gw")];
        assert_eq!(filter_options(&all, "K8S").len(), 2);
        assert_eq!(filter_options(&all, " ").len(), 3);
        let s = toggle(&["a".into()], "b", true);
        assert_eq!(s, ["a", "b"]);
        assert_eq!(toggle(&s, "a", true), ["b"]);
        assert_eq!(toggle(&s, "c", false), ["c"]);
    }
}
