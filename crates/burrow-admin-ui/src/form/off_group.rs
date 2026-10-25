use burrow_admin_logic::off::{group_line, groups};
use burrow_admin_logic::relations::field_state;
use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Node;
use leptos::prelude::*;

use crate::store::store;

pub(super) struct Off {
    pub reason: String,
    pub node: Node,
    pub base: Path,
    pub counted: bool,
}

pub(super) fn off_of(node: &Node, base: Path) -> Option<Off> {
    burrow_admin_logic::off::reason_of(node).map(|r| Off {
        reason: r.to_string(),
        node: node.clone(),
        base,
        counted: true,
    })
}

pub(super) fn beside(off: &Option<Off>) -> Option<Off> {
    off.as_ref().map(|o| Off {
        reason: o.reason.clone(),
        node: o.node.clone(),
        base: o.base.clone(),
        counted: false,
    })
}

pub(super) fn assemble(items: Vec<(Option<Off>, AnyView)>) -> Vec<AnyView> {
    let keys: Vec<(Option<String>, ())> = items
        .iter()
        .map(|(o, _)| (o.as_ref().map(|o| o.reason.clone()), ()))
        .collect();
    let counted = |j: &usize| items[*j].0.as_ref().is_some_and(|o| o.counted);
    let found: Vec<(String, Vec<usize>)> = groups(&keys)
        .into_iter()
        .filter(|(_, v)| v.iter().filter(|j| counted(j)).count() >= 2)
        .collect();
    let mut slots: Vec<Option<(Option<Off>, AnyView)>> = items.into_iter().map(Some).collect();
    let mut out = Vec::new();
    for i in 0..slots.len() {
        match found.iter().find(|(_, v)| v.first() == Some(&i)) {
            Some((_, members)) => {
                let mut first = None;
                let mut rows = Vec::new();
                let mut n = 0;
                for &j in members {
                    if let Some((o, v)) = slots[j].take() {
                        n += usize::from(o.as_ref().is_some_and(|o| o.counted));
                        first = first.or(o);
                        rows.push(v);
                    }
                }
                if let Some(off) = first {
                    out.push(group_view(off, n, rows));
                }
            }
            None => {
                if let Some((_, v)) = slots[i].take() {
                    out.push(v);
                }
            }
        }
    }
    out
}

fn group_view(off: Off, n: usize, rows: Vec<AnyView>) -> AnyView {
    let s = store();
    let e = s.edit;
    let schema = s.schema.get_untracked();
    let Off {
        reason, node, base, ..
    } = off;
    let line = group_line(n, &reason);
    let off = Memo::new(move |_| {
        schema.as_ref().is_some_and(|sc| {
            e.draft.with(|d| {
                field_state(&node, &sc.root, d.draft(), &base)
                    .disabled_reason
                    .as_deref()
                    == Some(reason.as_str())
            })
        })
    });
    view! {
        <details class="md:col-span-2" prop:open=move || !off.get() data-off-group="">
            <summary
                class="cursor-pointer py-2.5 text-[13px] font-semibold text-text-muted marker:text-text-faint"
                class:hidden=move || !off.get()
                data-testid="off-group"
            >
                {line}
            </summary>
            {rows}
        </details>
    }
    .into_any()
}
