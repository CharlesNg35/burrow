//! The `Combobox` component: a searchable single or multi pick list with chips, keyboard navigation and a popover that opens where it fits.
//! `create` adds making a new value from the typed text; `rows` renders the option list, at most `SHOWN` rows with a line for the rest.

mod create;
mod rows;

use std::collections::HashMap;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::basic::{Dot, Tone};
use crate::icon::Icon;

use create::{Create, create_section};
use rows::{Rows, SHOWN, keep_in_view, shown_len};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComboSize {
    #[default]
    Normal,
    Small,
}

impl ComboSize {
    fn field(self) -> &'static str {
        match self {
            ComboSize::Normal => "min-h-10 p-1 text-sm",
            ComboSize::Small => "min-h-7 p-px text-xs",
        }
    }

    fn input(self) -> &'static str {
        match self {
            ComboSize::Normal => "h-[26px]",
            ComboSize::Small => "h-6",
        }
    }

    fn chip(self) -> &'static str {
        match self {
            ComboSize::Normal => "h-[26px] text-sm",
            ComboSize::Small => "h-6 text-xs",
        }
    }
}

fn focus_by_id(id: &str) {
    if let Some(el) = document().get_element_by_id(id)
        && let Ok(el) = el.dyn_into::<web_sys::HtmlElement>()
    {
        crate::focus::focus_still(&el);
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComboOption {
    pub value: String,
    pub label: String,
    pub facet: String,
    pub dot: Option<Tone>,
    pub blocked: Option<String>,
    pub group: u8,
}

pub fn pickable(options: &[ComboOption], value: &str) -> bool {
    options
        .iter()
        .find(|o| o.value == value)
        .is_none_or(|o| o.blocked.is_none())
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

pub fn lift_selected(mut list: Vec<ComboOption>, selected: &[String]) -> Vec<ComboOption> {
    if let Some(at) = list.iter().position(|o| selected.contains(&o.value))
        && at >= SHOWN
    {
        let o = list.remove(at);
        list.insert(0, o);
    }
    list
}

pub fn start_at(visible: &[ComboOption], selected: &[String]) -> usize {
    visible
        .iter()
        .take(SHOWN)
        .position(|o| selected.contains(&o.value))
        .unwrap_or(0)
}

pub fn next_index(at: usize, len: usize, down: bool) -> usize {
    let a = at.min(len.saturating_sub(1));
    match (down, a) {
        (_, _) if len == 0 => 0,
        (true, _) => (a + 1) % len,
        (false, 0) => len - 1,
        (false, _) => a - 1,
    }
}

const POPOVER_ROOM: f64 = 240.0;
const POPOVER_MAX: f64 = 520.0;
const EDGE: f64 = 16.0;

fn view_width() -> f64 {
    window()
        .inner_width()
        .ok()
        .and_then(|w| w.as_f64())
        .unwrap_or(POPOVER_MAX)
}

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
    #[prop(optional)] size: ComboSize,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let creating = RwSignal::new(false);
    let new_name = RwSignal::new(String::new());
    let create_error: RwSignal<Option<String>> = RwSignal::new(None);
    let query = RwSignal::new(String::new());
    let active = RwSignal::new(0usize);
    let list_id = format!("{id}-list");
    let visible = Memo::new(move |_| {
        let found = options.with(|o| filter_options(o, &query.get()));
        if multi {
            found
        } else {
            selected.with(|s| lift_selected(found, s))
        }
    });
    let pick = move |value: String| {
        if !options.with_untracked(|o| pickable(o, &value)) {
            return;
        }
        let next = toggle(&selected.get_untracked(), &value, multi);
        on_change.run(next);
        query.set(String::new());
        if !multi {
            open.set(false);
        }
    };
    let remove = move |value: String| remove_value(selected, on_change, value);
    let chips = chips(options, selected, (multi, clearable, size), remove);
    let lit = Selector::new(move || {
        let n = visible.with(|v| shown_len(v));
        active.get().min(n.saturating_sub(1))
    });
    let rows = Rows {
        id: id.clone(),
        selected,
        visible,
        active,
        lit,
        pick,
        multi,
    }
    .view();
    let input_id = id.clone();
    let up = RwSignal::new(false);
    place_popover(id.clone(), open, up);
    let base = StoredValue::new(id.clone());
    Effect::new(move |was: Option<bool>| {
        let now = open.get();
        if now && was != Some(true) {
            let at = visible.with_untracked(|v| selected.with_untracked(|s| start_at(v, s)));
            active.set(at);
            request_animation_frame(move || base.with_value(|id| keep_in_view(id, at)));
        }
        now
    });
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
    let nav = Nav {
        open,
        active,
        query,
        visible,
        selected,
        multi,
        base,
    };
    let lid = list_id.clone();
    let list_label = [label.as_str(), header.as_str()]
        .into_iter()
        .find(|l| !l.is_empty())
        .map(str::to_string);
    let act_id = {
        let id = id.clone();
        move || {
            let n = visible.with(|v| shown_len(v));
            (open.get() && n > 0).then(|| format!("{id}-opt-{}", active.get().min(n - 1)))
        }
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
                    "flex items-center gap-[5px] rounded-input border bg-input {} {} {}",
                    size.field(),
                    if multi { "flex-wrap" } else { "flex-nowrap" },
                    if invalid.get() { "border-danger ring-3 ring-danger-weak" } else { "border-border-strong focus-within:border-accent focus-within:ring-3 focus-within:ring-accent-weak" }
                )
                on:click=move |_| {
                    focus_by_id(&input_id);
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
                    class=format!("{} {} flex-1 bg-transparent px-1.5 outline-none placeholder:text-text-faint", size.input(), if multi { "min-w-24" } else { "w-6 min-w-6" })
                    placeholder=move || if multi || selected.with(|s| s.iter().all(String::is_empty)) { placeholder.clone() } else { String::new() }
                    aria-label=(!label.is_empty()).then_some(label.clone())
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
                    on:keydown=move |e| on_key(&e, nav, &pick, &remove)
                />
                {(!multi).then(|| view! { <span class="pointer-events-none mr-1.5 ml-auto text-text-muted" aria-hidden="true" data-single=""><Icon name="chevron-down" small=true/></span> })}
            </div>
            <Show when=move || open.get()>
                <div class=move || format!("absolute left-0 z-30 w-full sm:w-max sm:min-w-64 sm:max-w-[var(--pop-room,100%)] rounded-[10px] border border-border-strong bg-surface p-[5px] shadow-overlay dark:bg-surface-2 {}", if up.get() { "bottom-full mb-1" } else { "top-full mt-1" })>
                    {(!header.is_empty()).then(|| view! { <div class="px-2.5 pt-1.5 pb-1 text-2xs font-bold tracking-[0.08em] text-text-faint uppercase">{header.clone()}</div> })}
                    <ul id=list_id.clone() role="listbox" aria-label=list_label.clone() aria-multiselectable=multi.then_some("true") class="max-h-72 overflow-y-auto">
                        {rows.clone()}
                    </ul>
                    {on_create.map(|cb| create_section(cb, create_label.clone(), &id, c, selected, on_change, multi))}
                </div>
            </Show>
        </div>
    }
}

#[derive(Clone, Copy)]
struct Nav {
    open: RwSignal<bool>,
    active: RwSignal<usize>,
    query: RwSignal<String>,
    visible: Memo<Vec<ComboOption>>,
    selected: Signal<Vec<String>>,
    multi: bool,
    base: StoredValue<String>,
}

impl Nav {
    fn step(self, down: bool) {
        let len = self.visible.with(|v| shown_len(v));
        if len == 0 {
            return;
        }
        let next = next_index(self.active.get_untracked(), len, down);
        self.active.set(next);
        self.base.with_value(|id| keep_in_view(id, next));
    }
}

fn on_key(e: &web_sys::KeyboardEvent, n: Nav, pick: &dyn Fn(String), remove: &dyn Fn(String)) {
    match e.key().as_str() {
        "ArrowDown" => {
            e.prevent_default();
            n.open.set(true);
            n.step(true);
        }
        "ArrowUp" => {
            e.prevent_default();
            n.step(false);
        }
        "Enter" => {
            e.prevent_default();
            if let Some(o) = n.visible.with(|v| v.get(n.active.get_untracked()).cloned()) {
                pick(o.value);
            }
        }
        "Escape" => {
            if n.open.get_untracked() {
                e.stop_propagation();
                n.open.set(false);
            }
        }
        "Backspace" if n.multi && n.query.with_untracked(String::is_empty) => {
            if let Some(last) = n.selected.get_untracked().last().cloned() {
                remove(last);
            }
        }
        _ => {}
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
    (multi, clearable, size): (bool, bool, ComboSize),
    remove: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl Fn() -> AnyView + Send + Sync + 'static {
    let labels = Memo::new(move |_| {
        options.with(|o| {
            o.iter()
                .map(|x| (x.value.clone(), (x.label.clone(), x.dot)))
                .collect::<HashMap<_, _>>()
        })
    });
    let label_of = move |v: &str| labels.with(|m| m.get(v).cloned());
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
                <span class=format!("inline-flex max-w-full min-w-0 items-center gap-1.5 rounded-sm pr-1 pl-2 font-semibold {} {look}", size.chip()) data-chip=v.clone() title=text.clone()>
                    <Dot tone=dot.unwrap_or(Tone::Neutral)/>
                    <span class="truncate">{text.clone()}</span>
                    {removable.then(|| view! {
                        <button
                            type="button"
                            class="relative flex size-[18px] cursor-pointer items-center justify-center rounded-xs text-text-faint after:absolute after:-inset-[3px] hover:bg-border hover:text-text"
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

fn place_popover(id: String, open: RwSignal<bool>, up: RwSignal<bool>) {
    let place = move || {
        let Some(field) = document().get_element_by_id(&id) else {
            return;
        };
        let floor = crate::bar::bar_top().unwrap_or_else(crate::bar::view_height);
        let r = field.get_bounding_client_rect();
        let below = floor - r.bottom();
        let above = r.top() - 72.0;
        up.set(below < POPOVER_ROOM && above > below);
        if let Some(el) = field.closest(".relative").ok().flatten()
            && let Ok(el) = el.dyn_into::<web_sys::HtmlElement>()
        {
            let b = el.get_bounding_client_rect();
            let wide = (view_width() - b.left() - EDGE).min(POPOVER_MAX);
            let _ = el.style().set_property("--pop-room", &format!("{wide}px"));
        }
    };
    let again = place.clone();
    Effect::new(move |_| {
        if open.get() {
            place();
        }
    });
    let resize = window_event_listener(leptos::ev::resize, move |_| {
        if open.get_untracked() {
            again();
        }
    });
    on_cleanup(move || resize.remove());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(v: &str) -> ComboOption {
        ComboOption {
            value: v.into(),
            label: v.into(),
            ..ComboOption::default()
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

    #[test]
    fn opening_starts_at_the_selected_option() {
        let list = vec![opt("self"), opt("home"), opt("office")];
        assert_eq!(start_at(&list, &["office".into()]), 2);
        assert_eq!(start_at(&list, &[]), 0);
        assert_eq!(start_at(&list, &["gone".into()]), 0);
    }

    #[test]
    fn arrows_wrap_around_like_the_apg_combobox() {
        assert_eq!(next_index(0, 5, true), 1);
        assert_eq!(next_index(4, 5, true), 0);
        assert_eq!(next_index(0, 5, false), 4);
        assert_eq!(next_index(9, 5, false), 3);
        assert_eq!(next_index(0, 0, true), 0);
    }

    #[test]
    fn a_long_list_shows_the_first_rows_and_counts_the_rest() {
        let many: Vec<ComboOption> = (0..500).map(|i| opt(&format!("app{i:03}"))).collect();
        assert_eq!(shown_len(&many), SHOWN);
        assert_eq!(
            rows::more_text(many.len()).as_deref(),
            Some("400 more, type to narrow")
        );
        assert_eq!(rows::more_text(SHOWN), None);
        assert_eq!(start_at(&many, &["app450".into()]), 0);
        assert_eq!(start_at(&many, &["app042".into()]), 42);
        assert_eq!(shown_len(&filter_options(&many, "app04")), 10);
        let lifted = lift_selected(many.clone(), &["app450".into()]);
        assert_eq!(lifted[0].value, "app450");
        assert_eq!(lifted.len(), 500);
        assert_eq!(start_at(&lifted, &["app450".into()]), 0);
        assert_eq!(
            lift_selected(many.clone(), &["app042".into()])[0].value,
            "app000"
        );
    }

    #[test]
    fn a_blocked_option_cannot_be_picked() {
        let mut off = opt("self");
        off.blocked = Some("not on yet".into());
        let all = vec![off, opt("home")];
        assert!(!pickable(&all, "self"));
        assert!(pickable(&all, "home"));
        assert!(pickable(&all, "gone"));
    }
}
