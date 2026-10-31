use std::collections::BTreeMap;

use burrow_admin_proto::serde_json::Value;
use burrow_config::ServiceEntry;
use burrow_ui_kit::TextInput;
use leptos::prelude::*;

use super::entries::{Place, sibling, text_of, typed};
use crate::edit::diags_at;
use crate::store::store;

pub(super) type Open = RwSignal<BTreeMap<usize, Option<Value>>>;

pub(super) fn shifted<V: Clone>(
    map: &BTreeMap<usize, V>,
    from: usize,
    by: isize,
) -> BTreeMap<usize, V> {
    map.iter()
        .filter_map(|(&k, v)| match k.cmp(&from) {
            std::cmp::Ordering::Less => Some((k, v.clone())),
            _ => k.checked_add_signed(by).map(|k| (k, v.clone())),
        })
        .collect()
}

pub(super) fn swapped<V: Clone>(
    map: &BTreeMap<usize, V>,
    a: usize,
    b: usize,
) -> BTreeMap<usize, V> {
    map.iter()
        .map(|(&k, v)| {
            let k = if k == a {
                b
            } else if k == b {
                a
            } else {
                k
            };
            (k, v.clone())
        })
        .collect()
}

pub(super) fn input_id(place: &Place, i: usize) -> String {
    format!("{}-{i}-target", place.fc.id)
}

pub(super) fn focus_soon(id: String) {
    request_animation_frame(move || {
        use wasm_bindgen::JsCast;
        if let Some(el) = document()
            .get_element_by_id(&id)
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
        }
    });
}

pub(super) fn to_main(place: &Place, i: usize, open: Open) {
    place.edit(|all| all[i].target = None);
    open.update(|o| {
        o.remove(&i);
    });
}

pub(super) fn main_shown(place: &Place) -> Signal<Option<String>> {
    let e = store().edit;
    let main_path = sibling(&place.fc.path, ServiceEntry::TARGET.rel());
    Signal::derive(move || {
        e.value(&main_path)
            .map(|v| burrow_admin_logic::web::target_value_shown(&v))
            .filter(|t| !t.is_empty())
    })
}

fn change_link(on: impl Fn() + 'static) -> impl IntoView {
    view! {
        <button type="button" class=format!("cursor-pointer text-sm font-semibold {}", burrow_ui_kit::LINK) data-testid="entry-change" on:click=move |_| on()>"Change"</button>
    }
}

pub(super) fn target_cell(place: &Place, i: usize, open: Open) -> impl IntoView {
    let e = store().edit;
    let id = input_id(place, i);
    let main = main_shown(place);
    let stored = {
        let place = place.clone();
        Memo::new(move |_| place.read().get(i).and_then(|r| r.target.clone()))
    };
    let target = Signal::derive(move || text_of(&stored.get()));
    let editing = Memo::new(move |_| open.with(|o| o.contains_key(&i)));
    let mut at = place.fc.path.clone();
    at.extend([
        i.to_string(),
        burrow_config::ServiceClient::TARGET.key().to_string(),
    ]);
    let invalid = Signal::derive(move || {
        e.plan
            .with(|p| !diags_at(p.as_deref(), &at, true).is_empty())
    });
    let set_target = {
        let place = place.clone();
        Callback::new(move |t: String| place.edit(|all| all[i].target = typed(&t)))
    };
    let close = move || {
        if !invalid.get_untracked() {
            open.update(|o| {
                o.remove(&i);
            });
        }
    };
    let escape = {
        let place = place.clone();
        move || {
            let before = open.with_untracked(|o| o.get(&i).cloned().flatten());
            place.edit(|all| all[i].target = before);
            open.update(|o| {
                o.remove(&i);
            });
        }
    };
    let clear = {
        let place = place.clone();
        Callback::new(move |_: ()| to_main(&place, i, open))
    };
    let start = {
        let id = id.clone();
        move || {
            open.update(|o| {
                o.insert(i, stored.get_untracked());
            });
            focus_soon(id.clone());
        }
    };
    let placeholder = main
        .get_untracked()
        .map(|m| format!("{} {m}", burrow_admin_logic::wording::MAIN_TARGET))
        .unwrap_or_else(|| format!("the {}", burrow_admin_logic::wording::MAIN_TARGET));
    let client = place
        .read()
        .get(i)
        .map(|r| r.name.clone())
        .unwrap_or_default();
    let other_label = format!(
        "{} for {client}",
        burrow_admin_logic::wording::capital(burrow_admin_logic::wording::OTHER_TARGET)
    );
    move || {
        if editing.get() {
            let escape = escape.clone();
            view! {
                <div class="flex min-w-0 flex-1 items-center gap-1" data-own-target=""
                    on:keydown=move |ev| match ev.key().as_str() {
                        "Enter" => { ev.prevent_default(); close(); }
                        "Escape" => { ev.prevent_default(); ev.stop_propagation(); escape(); }
                        _ => {}
                    }>
                    <div class="min-w-0 max-w-[280px] flex-1">
                        <TextInput id=id.clone() value=target on_input=set_target on_blur=Callback::new(move |_| close()) invalid=invalid mono=true placeholder=placeholder.clone() label=other_label.clone()/>
                    </div>
                    <span on:mousedown=|ev| ev.prevent_default()>
                        <burrow_ui_kit::IconButton icon="x" label=burrow_admin_logic::wording::USE_MAIN_TARGET testid="own-target-clear" on_click=clear/>
                    </span>
                </div>
            }
            .into_any()
        } else if let Some(own) = stored.get() {
            view! {
                <div class="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2 text-sm" data-own-text="">
                    <span class="font-mono text-[13px] text-text">{burrow_admin_logic::web::target_value_shown(&own)}</span>
                    {change_link(start.clone())}
                </div>
            }
            .into_any()
        } else {
            view! {
                <div class="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2 text-sm" data-main-target="">
                    <span class="text-text-muted">{format!("{} · ", burrow_admin_logic::wording::MAIN_TARGET)}{move || match main.get() {
                        Some(m) => view! { <span class="font-mono text-[13px]">{m}</span> }.into_any(),
                        None => view! { <span class="font-semibold text-warning" data-main-unset="">{burrow_admin_logic::wording::NOT_SET}</span> }.into_any(),
                    }}</span>
                    {change_link(start.clone())}
                </div>
            }
            .into_any()
        }
    }
}
