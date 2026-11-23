use std::collections::HashMap;

use burrow_ui_kit::{LABEL, VirtualList};
use leptos::prelude::*;

use super::Row;
use super::list::Look;
use super::list_rows::{Cards, full_card, print, still};

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

fn head_view(label: String, n: Option<usize>) -> AnyView {
    view! {
        <div class=format!("flex justify-between px-4 pt-3.5 pb-1.5 {LABEL}")>
            <span>{label}</span>
            {n.map(|n| view! { <span>{n}</span> })}
        </div>
    }
    .into_any()
}

pub(super) fn windowed(items: Memo<Vec<Item>>, look: Look, cards: Cards) -> AnyView {
    let estimate = if look.compact { GUESS_COMPACT } else { GUESS };
    let reveal = Memo::new(move |_| {
        cards.selected.with(|sel| {
            let sel = sel.as_deref()?;
            items.with(|it| it.iter().position(|i| i.row_key() == Some(sel)))
        })
    });
    let children = move |item: Item| match item {
        Item::Head { label, n } => head_view(label, n),
        Item::Card(r) => full_card(*r, cards),
    };
    view! {
        <VirtualList
            items=items
            key=Item::key
            children=children
            estimate=estimate
            scroll_in="[data-testid=list]"
            reveal=reveal
            testid="list-window"
        />
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

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
