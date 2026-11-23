use std::collections::HashMap;

use burrow_ui_kit::LABEL;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use super::Row;
use super::list::Look;
use super::list_rows::{Cards, full_card, print, still};

const OVERSCAN: f64 = 800.0;
const GUESS: f64 = 80.0;
const GUESS_COMPACT: f64 = 40.0;

#[derive(Clone, PartialEq)]
pub(super) enum Item {
    Head { label: String, n: Option<usize> },
    Card(Box<Row>),
}

impl Item {
    fn key(&self) -> String {
        match self {
            Item::Head { label, n } => format!("head:{label}:{n:?}"),
            Item::Card(r) => {
                let (k, h) = print(r);
                format!("{k}:{h}")
            }
        }
    }

    fn row_key(&self) -> Option<&str> {
        match self {
            Item::Card(r) => Some(&r.key),
            Item::Head { .. } => None,
        }
    }
}

pub(super) fn items(groups: &[String], visible: &[Row], counts: bool) -> Vec<Item> {
    let mut by: HashMap<&str, Vec<&Row>> = HashMap::new();
    for r in visible {
        by.entry(r.group.as_str()).or_default().push(r);
    }
    let several = by.len() > 1;
    let mut out = Vec::with_capacity(visible.len() + groups.len());
    for g in groups {
        let Some(rows) = by.get(g.as_str()) else {
            continue;
        };
        if !g.is_empty() {
            out.push(Item::Head {
                label: g.clone(),
                n: (several && counts).then_some(rows.len()),
            });
        }
        out.extend(rows.iter().map(|r| Item::Card(Box::new(still(r)))));
    }
    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Span {
    pub start: usize,
    pub end: usize,
    pub above: f64,
    pub below: f64,
}

pub(super) fn span(heights: &[f64], top: f64, view: f64) -> Span {
    let (from, to) = (top - OVERSCAN, top + view + OVERSCAN);
    let mut at = 0.0;
    let mut start = heights.len();
    let mut above = 0.0;
    let mut end = heights.len();
    for (i, h) in heights.iter().enumerate() {
        if start == heights.len() && at + h > from {
            start = i;
            above = at;
        }
        if at >= to {
            end = i;
            break;
        }
        at += h;
    }
    let total: f64 = heights.iter().sum();
    let shown: f64 = heights.get(start..end).map_or(0.0, |s| s.iter().sum());
    Span {
        start: start.min(end),
        end,
        above,
        below: (total - above - shown).max(0.0),
    }
}

fn offset_of(heights: &[f64], i: usize) -> f64 {
    heights.iter().take(i).sum()
}

type Watch = (
    web_sys::Element,
    Closure<dyn FnMut()>,
    Option<web_sys::ResizeObserver>,
);

struct Viewport {
    top: RwSignal<(f64, f64)>,
    ready: RwSignal<bool>,
    scroller: StoredValue<Option<web_sys::HtmlElement>, LocalStorage>,
}

fn viewport(me: NodeRef<html::Div>) -> Viewport {
    let top = RwSignal::new((0.0, 900.0));
    let ready = RwSignal::new(false);
    let scroller: StoredValue<Option<web_sys::HtmlElement>, LocalStorage> =
        StoredValue::new_local(None);
    let watch: StoredValue<Option<Watch>, LocalStorage> = StoredValue::new_local(None);
    Effect::new(move |_| {
        let Some(el) = me.get() else { return };
        let el: web_sys::HtmlElement = el.into();
        let Some(list) = el
            .closest("[data-testid=list]")
            .ok()
            .flatten()
            .and_then(|l| l.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            return;
        };
        let seen = list.clone();
        let read = move || {
            let now = (
                f64::from(seen.scroll_top()),
                f64::from(seen.client_height()),
            );
            if top.try_get_untracked().is_some_and(|t| t != now) {
                top.set(now);
            }
        };
        read();
        let on = Closure::<dyn FnMut()>::new(read);
        let _ = list.add_event_listener_with_callback("scroll", on.as_ref().unchecked_ref());
        let size = web_sys::ResizeObserver::new(on.as_ref().unchecked_ref()).ok();
        if let Some(o) = &size {
            o.observe(&list);
        }
        scroller.set_value(Some(list.clone()));
        ready.set(true);
        watch.set_value(Some((list.into(), on, size)));
    });
    on_cleanup(move || {
        watch.try_with_value(|w| {
            if let Some((el, on, size)) = w {
                let _ =
                    el.remove_event_listener_with_callback("scroll", on.as_ref().unchecked_ref());
                if let Some(o) = size {
                    o.disconnect();
                }
            }
        });
    });
    Viewport {
        top,
        ready,
        scroller,
    }
}

fn head_view(label: String, n: Option<usize>) -> AnyView {
    view! {
        <div class=format!("flex justify-between px-4 pt-3.5 pb-1.5 {LABEL}")>
            <span>{label}</span>
            {n.map(|n| view! { <span>{n}</span> })}
        </div>
    }
    .into_any()
}

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

pub(super) fn windowed(items: Memo<Vec<Item>>, look: Look, cards: Cards) -> AnyView {
    let me: NodeRef<html::Div> = NodeRef::new();
    let vp = viewport(me);
    let sizes: StoredValue<HashMap<String, f64>> = StoredValue::new(HashMap::new());
    let seen = RwSignal::new(0u64);
    let guess = if look.compact { GUESS_COMPACT } else { GUESS };
    let keys = Memo::new(move |_| items.with(|it| it.iter().map(Item::key).collect::<Vec<_>>()));
    let heights = Memo::new(move |_| {
        seen.track();
        keys.with(|k| {
            sizes.with_value(|m| {
                k.iter()
                    .map(|k| m.get(k).copied().unwrap_or(guess))
                    .collect::<Vec<_>>()
            })
        })
    });
    let range = Memo::new(move |_| {
        let (top, view) = vp.top.get();
        heights.with(|h| span(h, top, view))
    });
    let filled = Memo::new(move |_| items.with(|it| !it.is_empty()));
    Effect::new(move |_| {
        let Some(sel) = cards.selected.get() else {
            return;
        };
        if !filled.get() || !vp.ready.get() {
            return;
        }
        let at =
            items.with_untracked(|it| it.iter().position(|i| i.row_key() == Some(sel.as_str())));
        let (Some(at), Some(list)) = (at, vp.scroller.get_value()) else {
            return;
        };
        let r = range.get_untracked();
        if at >= r.start && at < r.end {
            return;
        }
        let y = heights.with_untracked(|h| offset_of(h, at));
        let to = (y - f64::from(list.client_height()) / 2.0).max(0.0);
        list.set_scroll_top(to as i32);
        vp.top.set((to, f64::from(list.client_height())));
    });
    Effect::new(move |_| {
        range.track();
        request_animation_frame(move || measure(me, sizes, seen));
    });
    let shown = move || {
        let r = range.get();
        items.with(|it| {
            it.get(r.start..r.end)
                .map(<[Item]>::to_vec)
                .unwrap_or_default()
        })
    };
    view! {
        <div node_ref=me data-testid="list-window">
            <div aria-hidden="true" style:height=move || format!("{}px", range.get().above)></div>
            <For
                each=shown
                key=Item::key
                children=move |item| {
                    let k = item.key();
                    let body = match item {
                        Item::Head { label, n } => head_view(label, n),
                        Item::Card(r) => full_card(*r, cards),
                    };
                    view! { <div class="flow-root" data-vkey=k>{body}</div> }
                }
            />
            <div aria-hidden="true" style:height=move || format!("{}px", range.get().below)></div>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_holds_the_rows_in_view_and_a_margin_around_them() {
        let h = vec![100.0; 1000];
        let s = span(&h, 50_000.0, 900.0);
        assert_eq!(s.start, 492);
        assert_eq!(s.end, 517);
        assert_eq!(s.above, 49_200.0);
        assert_eq!(
            s.above + (s.end - s.start) as f64 * 100.0 + s.below,
            100_000.0
        );
        let top = span(&h, 0.0, 900.0);
        assert_eq!((top.start, top.above), (0, 0.0));
        let all = span(&[50.0; 3], 0.0, 900.0);
        assert_eq!((all.start, all.end, all.below), (0, 3, 0.0));
        assert_eq!(
            span(&[], 0.0, 900.0),
            Span {
                start: 0,
                end: 0,
                above: 0.0,
                below: 0.0
            }
        );
    }

    #[test]
    fn groups_get_a_head_and_a_count_only_when_several_have_rows() {
        let row = |k: &str, g: &str| Row {
            key: k.into(),
            group: g.into(),
            ..Row::default()
        };
        let groups = vec!["Problems".to_string(), "Up".to_string()];
        let one = items(&groups, &[row("a", "Up"), row("b", "Up")], true);
        assert_eq!(one.len(), 3);
        assert!(matches!(&one[0], Item::Head { label, n: None } if label == "Up"));
        let two = items(&groups, &[row("a", "Up"), row("b", "Problems")], true);
        assert!(matches!(&two[0], Item::Head { label, n: Some(1) } if label == "Problems"));
        assert_eq!(two[1].row_key(), Some("b"));
        assert!(
            items(&groups, &[row("a", "Up"), row("b", "Problems")], false)
                .iter()
                .all(|i| !matches!(i, Item::Head { n: Some(_), .. }))
        );
    }
}
