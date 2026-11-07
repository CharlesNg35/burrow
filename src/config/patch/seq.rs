use serde_json::Value;

use super::super::Format;
use super::super::emit::render;
use super::super::tree::{self, Node};
use super::diff::{diff_map, scalar_text};
use super::{Edits, child, content_end, end_of, is_flow, line_end, line_start, scalar_end};

fn item_text(v: &Value, indent: &str) -> String {
    render(&Value::Array(vec![v.clone()]), Format::Yaml)
        .lines()
        .map(|l| format!("{indent}{l}\n"))
        .collect()
}

pub(super) fn patch_maps(
    src: &str,
    old: &Node,
    items: &[Value],
    path: &[String],
    (depth, unit): (usize, usize),
    edits: &mut Edits,
) {
    let tree::Value::Seq(old_items) = &old.value else {
        return;
    };
    let first = line_start(src, old_items[0].span.start.offset);
    let indent = &src[first..src[first..].find('-').map_or(first, |d| first + d)];
    if indent.chars().any(|c| c != ' ') {
        edits.give_up(path, "the list items do not start their lines");
        return;
    }
    let last = &old_items[old_items.len() - 1];
    let end = end_of(
        src,
        first,
        content_end(last).max(last.span.end.offset),
        indent.len(),
    );
    let starts: Vec<usize> = old_items
        .iter()
        .map(|it| line_start(src, it.span.start.offset))
        .collect();
    if starts.windows(2).any(|w| w[0] >= w[1]) {
        edits.give_up(path, "several list items share a line");
        return;
    }
    let range = |i: usize| (starts[i], starts.get(i + 1).copied().unwrap_or(end));
    for (i, it) in old_items.iter().enumerate().take(items.len()) {
        let want = &items[i];
        if &it.to_json() == want {
            continue;
        }
        let block = matches!(&it.value, tree::Value::Map(m) if !m.is_empty()) && !is_flow(src, it);
        if block && want.as_object().is_some_and(|m| !m.is_empty()) {
            diff_map(
                src,
                it,
                want,
                &child(path, &i.to_string()),
                depth + 1,
                unit,
                edits,
            );
        } else {
            let (from, to) = range(i);
            edits.push(from, to, item_text(want, indent));
        }
    }
    if items.len() < old_items.len() {
        edits.push(range(items.len()).0, end, String::new());
    }
    if items.len() > old_items.len() {
        let text: String = items[old_items.len()..]
            .iter()
            .map(|v| item_text(v, indent))
            .collect();
        let lead = if src[..end].ends_with('\n') { "" } else { "\n" };
        edits.push(end, end, format!("{lead}{text}"));
    }
}

pub(super) fn patch_flow_scalars(src: &str, old: &Node, want: &Value, edits: &mut Edits) -> bool {
    let (tree::Value::Map(entries), Some(wanted)) = (&old.value, want.as_object()) else {
        return false;
    };
    let same_keys =
        entries.len() == wanted.len() && entries.iter().all(|e| wanted.contains_key(&e.key));
    if !is_flow(src, old) || !same_keys {
        return false;
    }
    let changed: Vec<(&tree::Entry, &Value)> = entries
        .iter()
        .filter_map(|e| Some((e, wanted.get(&e.key)?)))
        .filter(|(e, v)| &e.value.to_json() != *v)
        .collect();
    let plain = changed.iter().all(|(e, v)| {
        !v.is_object()
            && !v.is_array()
            && !matches!(e.value.value, tree::Value::Map(_) | tree::Value::Seq(_))
            && e.value.span.start.line == e.value.span.end.line
            && line_end(src, e.value.span.start.offset) >= e.value.span.end.offset
    });
    if !plain {
        return false;
    }
    for (e, v) in changed {
        edits.push(
            e.value.span.start.offset,
            scalar_end(src, &e.value),
            scalar_text(src, &e.value, v),
        );
    }
    true
}
