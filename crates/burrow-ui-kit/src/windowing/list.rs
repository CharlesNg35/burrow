use std::collections::HashMap;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::range::{Snap, offset, span};
use super::track::{Scroller, track};

const AROUND: f64 = 800.0;

fn measure(me: NodeRef<html::Div>, sizes: StoredValue<HashMap<String, f64>>, seen: RwSignal<u64>) {
    let Some(el) = me.try_get_untracked().flatten() else {
        return;
    };
    let Ok(nodes) = el.query_selector_all("[data-vkey]") else {
        return;
    };
    let mut changed = false;
    sizes.update_value(|m| {
        for i in 0..nodes.length() {
            let Some(n) = nodes
                .get(i)
                .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
            else {
                continue;
            };
            let (Some(k), h) = (n.get_attribute("data-vkey"), f64::from(n.offset_height())) else {
                continue;
            };
            if h > 0.0 && m.get(&k).is_none_or(|o| (o - h).abs() > 0.5) {
                m.insert(k, h);
                changed = true;
            }
        }
    });
    if changed {
        seen.update(|n| *n += 1);
    }
}

#[component]
pub fn VirtualList<T, KF, CF, V>(
    items: Memo<Vec<T>>,
    key: KF,
    children: CF,
    estimate: f64,
    scroll_in: &'static str,
    #[prop(into)] reveal: Signal<Option<usize>>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView
where
    T: Clone + PartialEq + Send + Sync + 'static,
    KF: Fn(&T) -> String + Copy + Send + Sync + 'static,
    CF: Fn(T) -> V + Copy + Send + Sync + 'static,
    V: IntoView + 'static,
{
    let me: NodeRef<html::Div> = NodeRef::new();
    let tracked = track(Scroller::Closest(me, scroll_in));
    let sizes: StoredValue<HashMap<String, f64>> = StoredValue::new(HashMap::new());
    let seen = RwSignal::new(0u64);
    let keys = Memo::new(move |_| items.with(|it| it.iter().map(key).collect::<Vec<_>>()));
    let heights = Memo::new(move |_| {
        seen.track();
        keys.with(|k| {
            sizes.with_value(|m| {
                k.iter()
                    .map(|k| m.get(k).copied().unwrap_or(estimate))
                    .collect::<Vec<_>>()
            })
        })
    });
    let range = Memo::new(move |_| {
        let band = tracked.view.get().band(AROUND, Snap::Exact);
        heights.with(|h| span(h, band))
    });
    Effect::new(move |_| {
        let (Some(at), true) = (reveal.get(), tracked.ready.get()) else {
            return;
        };
        let r = range.get_untracked();
        if at >= r.start && at < r.end {
            return;
        }
        let y = heights.with_untracked(|h| offset(h, at));
        let half = tracked.view.get_untracked().height / 2.0;
        tracked.scroll_to(y - half);
    });
    Effect::new(move |_| {
        range.track();
        request_animation_frame(move || measure(me, sizes, seen));
    });
    let shown = move || {
        let r = range.get();
        items.with(|it| {
            it.get(r.start..r.end)
                .map(<[T]>::to_vec)
                .unwrap_or_default()
        })
    };
    view! {
        <div node_ref=me data-testid=(!testid.is_empty()).then_some(testid)>
            <div aria-hidden="true" style:height=move || format!("{}px", range.get().above)></div>
            <For
                each=shown
                key=move |t: &T| key(t)
                children=move |t: T| {
                    let k = key(&t);
                    view! { <div class="flow-root" data-vkey=k>{children(t)}</div> }
                }
            />
            <div aria-hidden="true" style:height=move || format!("{}px", range.get().below)></div>
        </div>
    }
}
