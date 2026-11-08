use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use burrow_ui_kit::{EmptyState, LABEL};
use leptos::prelude::*;

use crate::store::Grouping;

use super::list::{ListCard, Look, Shown, TocRow};
use super::{Row, file_groups};

pub(super) type LiveMap = HashMap<String, (String, String)>;

#[derive(Clone, Copy)]
pub(super) struct Cards {
    pub shown: RwSignal<HashSet<String>>,
    pub selected: Signal<Option<String>>,
    pub live: Memo<LiveMap>,
    pub compact: bool,
}

pub(super) fn still(r: &Row) -> Row {
    Row {
        meta: String::new(),
        line2: if r.line2.is_empty() {
            String::new()
        } else {
            " ".into()
        },
        ..r.clone()
    }
}

pub(super) fn print(r: &Row) -> (String, u64) {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{r:?}").hash(&mut h);
    (r.key.clone(), h.finish())
}

pub(super) fn card(r: Row, c: Cards) -> AnyView {
    let key = r.key.clone();
    let sel = key.clone();
    let on = Signal::derive(move || c.selected.with(|s| s.as_deref() == Some(sel.as_str())));
    let shown = key.clone();
    let hidden = Signal::derive(move || !c.shown.with(|s| s.contains(&shown)));
    if c.compact {
        return view! { <TocRow row=r selected=on hidden=hidden/> }.into_any();
    }
    let live = c.live;
    let mine = Memo::new(move |_| live.with(|m| m.get(&key).cloned().unwrap_or_default()));
    view! { <ListCard row=r selected=on live=mine hidden=hidden/> }.into_any()
}

pub(super) const CHUNK: usize = 120;

pub(super) fn next_batch(
    order: &[String],
    shown: &HashSet<String>,
    first: Option<&str>,
    n: usize,
) -> Vec<String> {
    let mut out: Vec<String> = first
        .filter(|k| !shown.contains(*k) && order.iter().any(|o| o == k))
        .map(|k| vec![k.to_string()])
        .unwrap_or_default();
    out.extend(
        order
            .iter()
            .filter(|k| !shown.contains(*k) && Some(k.as_str()) != first)
            .take(n.saturating_sub(out.len()))
            .cloned(),
    );
    out
}

fn revealed(
    visible: Memo<Vec<Row>>,
    selected: Signal<Option<String>>,
) -> RwSignal<HashSet<String>> {
    let shown = RwSignal::new(HashSet::new());
    let round = StoredValue::new(0u64);
    let step = move |me: u64| {
        if round.try_get_value() != Some(me) {
            return true;
        }
        let Some(order) =
            visible.try_with_untracked(|v| v.iter().map(|r| r.key.clone()).collect::<Vec<_>>())
        else {
            return true;
        };
        let sel = selected.try_get_untracked().flatten();
        let Some(batch) =
            shown.try_with_untracked(|s| next_batch(&order, s, sel.as_deref(), CHUNK))
        else {
            return true;
        };
        if batch.is_empty() {
            return true;
        }
        shown.try_update(|s| s.extend(batch)).is_none()
    };
    Effect::new(move |_| {
        let keep: HashSet<String> = visible.with(|v| v.iter().map(|r| r.key.clone()).collect());
        let me = round.get_value() + 1;
        round.set_value(me);
        shown.update(|s| s.retain(|k| keep.contains(k)));
        if step(me) {
            return;
        }
        fn later(step: impl Fn(u64) -> bool + Clone + 'static, me: u64) {
            request_animation_frame(move || {
                if !step(me) {
                    later(step, me);
                }
            });
        }
        later(step, me);
    });
    shown
}

fn group_names(groups: &[&'static str], all: &[Row]) -> Vec<String> {
    let mut gs: Vec<String> = groups.iter().map(|g| g.to_string()).collect();
    for r in all {
        if !gs.contains(&r.group) {
            gs.push(r.group.clone());
        }
    }
    gs
}

fn flat(
    all: Memo<Vec<Row>>,
    visible: Memo<Vec<Row>>,
    groups: Vec<&'static str>,
    look: Look,
    cards: Cards,
) -> AnyView {
    let names = Memo::new(move |_| all.with(|a| group_names(&groups, a)));
    let counts: Memo<HashMap<String, usize>> = Memo::new(move |_| {
        let mut m = HashMap::new();
        visible.with(|v| {
            for r in v {
                *m.entry(r.group.clone()).or_insert(0) += 1;
            }
        });
        m
    });
    let several = Memo::new(move |_| counts.with(|c| c.values().filter(|n| **n > 0).count() > 1));
    view! {
        <For each=move || names.get() key=|g| g.clone() let:g>
            {
                let name = g.clone();
                let n = Memo::new(move |_| counts.with(|c| c.get(&name).copied().unwrap_or(0)));
                let label = g.clone();
                let mine = g.clone();
                let rows = move || all.with(|a| a.iter().filter(|r| r.group == mine).cloned().collect::<Vec<_>>());
                view! {
                    {(!g.is_empty()).then(|| view! {
                        <div class=format!("flex justify-between px-4 pt-3.5 pb-1.5 {LABEL}") hidden=move || n.get() == 0>
                            <span>{label.clone()}</span>
                            {move || (several.get() && !look.no_counts).then(|| view! { <span>{n.get()}</span> })}
                        </div>
                    })}
                    <For each=rows key=print children=move |r| card(r, cards)/>
                }
            }
        </For>
    }
    .into_any()
}

pub(super) fn list_body(
    shown_in: Shown,
    empty: Option<(String, String)>,
    empty_icon: Option<&'static str>,
    groups: Vec<&'static str>,
    look: Look,
) -> AnyView {
    let Shown {
        visible,
        rows,
        selected,
        by_file,
        searching,
    } = shown_in;
    let all = Memo::new(move |_| rows.with(|r| r.iter().map(still).collect::<Vec<_>>()));
    let shown = revealed(visible, selected);
    let live: Memo<LiveMap> = Memo::new(move |_| {
        rows.with(|v| {
            v.iter()
                .map(|r| (r.key.clone(), (r.meta.clone(), r.line2.clone())))
                .collect()
        })
    });
    let cards = Cards {
        shown,
        selected,
        live,
        compact: look.compact,
    };
    let nothing = Memo::new(move |_| visible.with(Vec::is_empty));
    let none_at_all = Memo::new(move |_| rows.with(Vec::is_empty));
    let grouped = Memo::new(move |_| by_file.filter(|k| file_groups::mode(k) == Grouping::ByFile));
    let empty_view = move || {
        if !nothing.get() {
            return ().into_any();
        }
        match &empty {
            Some((t, _)) if t.is_empty() => ().into_any(),
            Some((t, m)) if none_at_all.get() => view! { <EmptyState icon=empty_icon.unwrap_or("plus") title=t.clone() message=m.clone() testid="list-empty"/> }.into_any(),
            _ => view! { <p class="px-4 py-6 text-sm text-text-muted">"Nothing matches."</p> }.into_any(),
        }
    };
    let body = move || match grouped.get() {
        Some(kind) => file_groups::render(kind, all, visible, searching, cards),
        None => flat(all, visible, groups.clone(), look, cards),
    };
    view! { {empty_view} {body} }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("k{i}")).collect()
    }

    #[test]
    fn a_big_reveal_comes_in_batches_with_the_selection_first() {
        let order = keys(300);
        let mut shown: HashSet<String> = HashSet::new();
        let first = next_batch(&order, &shown, Some("k250"), CHUNK);
        assert_eq!(first.len(), CHUNK);
        assert_eq!(
            first[0], "k250",
            "the selected row comes in the first batch"
        );
        assert_eq!(first[1], "k0");
        shown.extend(first);
        let second = next_batch(&order, &shown, Some("k250"), CHUNK);
        assert_eq!(second.len(), CHUNK);
        assert!(!second.contains(&"k250".to_string()));
        shown.extend(second);
        assert_eq!(
            next_batch(&order, &shown, None, CHUNK).len(),
            300 - 2 * CHUNK
        );
        shown.extend(order.clone());
        assert!(next_batch(&order, &shown, None, CHUNK).is_empty());
        assert_eq!(
            next_batch(&order, &HashSet::new(), Some("gone"), 2),
            ["k0", "k1"],
            "a selection that is not shown is not forced in"
        );
    }
}
