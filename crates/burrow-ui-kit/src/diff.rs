use std::sync::Arc;

use leptos::prelude::*;

use crate::more::{ShowMore, shown_of};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: char,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

const CHUNK: usize = 100;
const STEP: usize = 4 * CHUNK;

fn row_class(kind: char) -> &'static str {
    match kind {
        '+' => "bg-diff-add",
        '-' => "bg-diff-del",
        '@' => "bg-accent-weak text-accent-ink",
        _ => "",
    }
}

fn sign(kind: char) -> (&'static str, &'static str) {
    match kind {
        '+' => ("+", "text-success"),
        '-' => ("−", "text-danger"),
        _ => ("", ""),
    }
}

type Pair = (Option<DiffLine>, Option<DiffLine>);

pub fn side_by_side(all: &[DiffLine]) -> Vec<Pair> {
    let mut rows: Vec<Pair> = Vec::with_capacity(all.len());
    let (mut dels, mut adds): (Vec<DiffLine>, Vec<DiffLine>) = (Vec::new(), Vec::new());
    let flush = |rows: &mut Vec<Pair>, dels: &mut Vec<DiffLine>, adds: &mut Vec<DiffLine>| {
        let n = dels.len().max(adds.len());
        let mut d = std::mem::take(dels).into_iter();
        let mut a = std::mem::take(adds).into_iter();
        for _ in 0..n {
            rows.push((d.next(), a.next()));
        }
    };
    for l in all {
        match l.kind {
            '-' => dels.push(l.clone()),
            '+' => adds.push(l.clone()),
            _ => {
                flush(&mut rows, &mut dels, &mut adds);
                rows.push((Some(l.clone()), Some(l.clone())));
            }
        }
    }
    flush(&mut rows, &mut dels, &mut adds);
    rows
}

fn digits(lines: &[DiffLine]) -> usize {
    let top = lines
        .iter()
        .flat_map(|l| [l.old, l.new])
        .flatten()
        .max()
        .unwrap_or(0);
    top.max(1).ilog10() as usize + 1
}

fn num(n: Option<u32>) -> String {
    n.map(|n| n.to_string()).unwrap_or_default()
}

fn unified_row(l: &DiffLine, cols: &str) -> AnyView {
    let (s, sc) = sign(l.kind);
    view! {
        <div class=format!("grid {}", row_class(l.kind)) style=format!("grid-template-columns:{cols}") role="row">
            <span class="pr-2 text-right text-text-faint select-none" role="cell">{num(l.old)}</span>
            <span class="pr-2 text-right text-text-faint select-none" role="cell">{num(l.new)}</span>
            <span class=format!("text-center select-none {sc}") role="cell">{s}</span>
            <span class="pr-3 whitespace-pre" role="cell">{l.text.clone()}</span>
        </div>
    }
    .into_any()
}

fn half(l: Option<&DiffLine>, old: bool, cols: &str) -> AnyView {
    match l {
        Some(l) => {
            let n = if old { l.old } else { l.new };
            let (s, sc) = sign(l.kind);
            view! {
                <div class=format!("grid {}", row_class(l.kind)) style=format!("grid-template-columns:{cols}") role="cell">
                    <span class="pr-2 text-right text-text-faint select-none">{num(n)}</span>
                    <span class=format!("text-center select-none {sc}")>{s}</span>
                    <span class="pr-3 whitespace-pre">{l.text.clone()}</span>
                </div>
            }
            .into_any()
        }
        None => view! { <div role="cell"></div> }.into_any(),
    }
}

enum Rows {
    Unified(Vec<DiffLine>),
    Split(Vec<Pair>),
}

impl Rows {
    fn len(&self) -> usize {
        match self {
            Rows::Unified(l) => l.len(),
            Rows::Split(p) => p.len(),
        }
    }

    fn view(&self, from: usize, to: usize, gutter: &str) -> AnyView {
        match self {
            Rows::Unified(l) => {
                let cols = format!("{gutter} {gutter} 18px 1fr");
                l[from..to]
                    .iter()
                    .map(|l| unified_row(l, &cols))
                    .collect_view()
                    .into_any()
            }
            Rows::Split(p) => {
                let cols = format!("{gutter} 18px 1fr");
                p[from..to]
                    .iter()
                    .map(|(a, b)| view! { <div class="grid grid-cols-2 divide-x divide-border" role="row">{half(a.as_ref(), true, &cols)}{half(b.as_ref(), false, &cols)}</div> })
                    .collect_view()
                    .into_any()
            }
        }
    }
}

fn chunks(shown: usize) -> Vec<usize> {
    (0..shown.div_ceil(CHUNK)).collect()
}

#[component]
fn DiffRows(rows: Arc<Rows>, gutter: String) -> impl IntoView {
    let total = rows.len();
    let shown = RwSignal::new(STEP);
    let gutter = Arc::new(gutter);
    view! {
        <div class="overflow-x-auto font-mono text-[12.5px] leading-[1.8]" role="table" aria-label="Changed lines">
        <div role="rowgroup">
            <For
                each=move || chunks(shown_of(total, shown.get()))
                key=|i| *i
                children=move |i| {
                    let (from, to) = (i * CHUNK, total.min((i + 1) * CHUNK));
                    view! { <div data-diff-chunk="">{rows.view(from, to, &gutter)}</div> }
                }
            />
        </div>
        </div>
        <ShowMore shown=shown total=total step=STEP noun="lines" class="border-t border-border" testid="diff-more"/>
    }
}

#[component]
pub fn DiffView(
    #[prop(into)] lines: Signal<Vec<DiffLine>>,
    #[prop(optional, into)] split: Signal<bool>,
    #[prop(optional)] testid: &'static str,
) -> impl IntoView {
    let body = move || {
        let all = lines.get();
        let gutter = format!("calc({}ch + 0.75rem)", digits(&all).max(3));
        let rows = if split.get() {
            Rows::Split(side_by_side(&all))
        } else {
            Rows::Unified(all)
        };
        view! { <DiffRows rows=Arc::new(rows) gutter=gutter/> }
    };
    view! {
        <div
            class="overflow-hidden rounded-card border border-border bg-surface"
            data-testid=(!testid.is_empty()).then_some(testid)
        >
            {body}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{CHUNK, DiffLine, chunks, digits, side_by_side};

    fn line(kind: char, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            old,
            new,
            text: String::new(),
        }
    }

    #[test]
    fn side_by_side_pairs_removed_with_added_lines() {
        let all = [
            line(' ', Some(1), Some(1)),
            line('-', Some(2), None),
            line('-', Some(3), None),
            line('+', None, Some(2)),
            line(' ', Some(4), Some(3)),
        ];
        let rows = side_by_side(&all);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].1.as_ref().and_then(|l| l.new), Some(2));
        assert!(rows[2].1.is_none());
    }

    #[test]
    fn the_gutter_fits_the_largest_line_number_and_rows_come_in_chunks() {
        assert_eq!(digits(&[line(' ', Some(9), Some(31250))]), 5);
        assert_eq!(digits(&[]), 1);
        assert_eq!(chunks(0).len(), 0);
        assert_eq!(chunks(CHUNK + 1), vec![0, 1]);
    }
}
