use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::basic::{Dot, Tone};
use crate::icon::Icon;

use super::ComboOption;

pub const SHOWN: usize = 100;

pub fn shown_len(visible: &[ComboOption]) -> usize {
    visible.len().min(SHOWN)
}

pub fn more_text(total: usize) -> Option<String> {
    (total > SHOWN).then(|| format!("{} more, type to narrow", total - SHOWN))
}

pub(super) fn keep_in_view(id: &str, at: usize) {
    let doc = document();
    let (Some(list), Some(row)) = (
        doc.get_element_by_id(&format!("{id}-list")),
        doc.get_element_by_id(&format!("{id}-opt-{at}")),
    ) else {
        return;
    };
    let (l, r) = (
        list.get_bounding_client_rect(),
        row.get_bounding_client_rect(),
    );
    let shift = if r.top() < l.top() {
        r.top() - l.top()
    } else if r.bottom() > l.bottom() {
        r.bottom() - l.bottom()
    } else {
        return;
    };
    list.set_scroll_top(list.scroll_top() + shift.round() as i32);
}

#[derive(Clone)]
pub(super) struct Rows<P> {
    pub(super) id: String,
    pub(super) selected: Signal<Vec<String>>,
    pub(super) visible: Memo<Vec<ComboOption>>,
    pub(super) active: RwSignal<usize>,
    pub(super) lit: Selector<usize>,
    pub(super) pick: P,
    pub(super) multi: bool,
    pub(super) pointer: StoredValue<(i32, i32)>,
}

impl<P: Fn(String) + Copy + Send + Sync + 'static> Rows<P> {
    pub(super) fn view(self) -> impl Fn() -> AnyView + Clone + Send + Sync + 'static {
        move || {
            self.visible.with(|list| {
                if list.is_empty() {
                    return view! { <li class="px-2.5 py-2 text-sm text-text-muted" role="presentation">"Nothing matches"</li> }.into_any();
                }
                let more = more_text(list.len()).map(|m| view! { <li class="px-2.5 pt-2 pb-1.5 text-xs text-text-muted" role="presentation" data-more="">{m}</li> });
                let items = list
                    .iter()
                    .take(SHOWN)
                    .enumerate()
                    .map(|(i, o)| self.row(i, o, i > 0 && list[i - 1].group != o.group))
                    .collect_view();
                view! { {items}{more} }.into_any()
            })
        }
    }

    fn row(&self, i: usize, o: &ComboOption, split: bool) -> impl IntoView + use<P> {
        let (selected, active, pick, multi, pointer) = (
            self.selected,
            self.active,
            self.pick,
            self.multi,
            self.pointer,
        );
        let lit = self.lit.clone();
        let on = {
            let v = o.value.clone();
            move || selected.with(|s| s.contains(&v))
        };
        let value = o.value.clone();
        let off = o.blocked.is_some();
        let tip = o.blocked.clone().unwrap_or_else(|| o.facet.clone());
        let check = {
            let on = on.clone();
            move || {
                (!multi && on()).then(|| view! { <span class="ml-auto text-accent-ink sm:ml-0"><Icon name="check" small=true/></span> })
            }
        };
        let aria = on.clone();
        view! {
            {split.then(|| view! { <li role="separator" class="mx-2.5 my-1 h-px bg-border"></li> })}
            <li
                id=format!("{}-opt-{i}", self.id)
                role="option"
                aria-selected=move || aria().to_string()
                aria-disabled=off.then_some("true")
                title=(!tip.is_empty()).then_some(tip)
                class="flex min-h-[34px] flex-wrap items-center gap-x-2.5 rounded-input px-2.5 py-1.5 text-sm whitespace-nowrap sm:flex-nowrap sm:py-0"
                class:cursor-pointer=!off
                class:text-text-muted=off
                class:cursor-not-allowed=off
                class:bg-surface-3=move || lit.selected(&i)
                data-option=o.value.clone()
                on:mousedown=move |e| e.prevent_default()
                on:mousemove=move |e| {
                    let at = (e.client_x(), e.client_y());
                    if pointer.get_value() != at {
                        pointer.set_value(at);
                        active.set(i);
                    }
                }
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
                <Dot tone=o.dot.unwrap_or(Tone::Neutral)/>
                <span class="max-w-[calc(100%-18px)] shrink-0 truncate font-semibold">{o.label.clone()}</span>
                <span data-facet="" class="order-last basis-full truncate pl-[18px] text-[11.5px] text-text-muted sm:order-none sm:ml-auto sm:min-w-0 sm:basis-auto sm:pl-3">{o.facet.clone()}</span>
                {check}
            </li>
        }
    }
}
