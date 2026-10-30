use serde_json::Value;

use super::super::Format;
use super::super::emit::{inline, render, scalar_str};
use super::super::tree::{self, Node};
use super::{
    Edits, child, comment_at, end_of, entry_bounds, entry_yaml, is_flow, lead, line_end, line_start,
};

pub(super) fn diff_map(
    src: &str,
    node: &Node,
    target: &Value,
    path: &[String],
    depth: usize,
    unit: usize,
    edits: &mut Edits,
) {
    let tree::Value::Map(entries) = &node.value else {
        edits.give_up(path, "a mapping was expected");
        return;
    };
    let Some(wanted) = target.as_object() else {
        edits.give_up(path, "a mapping was expected");
        return;
    };
    let first = line_start(src, entries[0].key_span.start.offset);
    let indent = &src[first..entries[0].key_span.start.offset];
    if indent.chars().any(|c| c != ' ') {
        edits.give_up(path, "the keys do not start their lines");
        return;
    }
    let increasing = entries.windows(2).all(|w| {
        line_start(src, w[0].key_span.start.offset) < line_start(src, w[1].key_span.start.offset)
            && w[0].value.span.end.offset <= w[1].key_span.start.offset
    });
    if !increasing {
        edits.give_up(path, "several keys share a line");
        return;
    }
    let bounds = entry_bounds(src, entries, indent);
    let kept = entries
        .iter()
        .filter(|e| wanted.contains_key(&e.key))
        .count();
    let added: Vec<(&String, &Value)> = wanted
        .iter()
        .filter(|(k, _)| !entries.iter().any(|e| &e.key == *k))
        .collect();
    if kept == 0 && added.is_empty() {
        edits.give_up(path, "the mapping becomes empty");
        return;
    }
    for (i, e) in entries.iter().enumerate() {
        match wanted.get(&e.key) {
            None => {
                let (start, end) = bounds[i];
                edits.push(lead(src, start, indent), end, String::new());
            }
            Some(v) if &e.value.to_json() == v => {}
            Some(v) => diff_value(src, e, v, &child(path, &e.key), depth + 1, unit, edits),
        }
    }
    if !added.is_empty() {
        let after = entries
            .iter()
            .enumerate()
            .rev()
            .find(|(_, e)| wanted.contains_key(&e.key))
            .map(|(i, _)| bounds[i].1)
            .unwrap_or(bounds[entries.len() - 1].1);
        let text: String = added
            .iter()
            .map(|(k, v)| entry_yaml(k, v, indent))
            .collect();
        let lead = if after == 0 || src[..after].ends_with('\n') {
            ""
        } else {
            "\n"
        };
        edits.push(after, after, format!("{lead}{text}"));
    }
}

fn quoted_like(src: &str, old: &Node, s: &str) -> String {
    let first = src[old.span.start.offset..].chars().next();
    match first {
        Some('\'') if !s.contains('\n') => format!("'{}'", s.replace('\'', "''")),
        Some('"') => serde_json::to_string(s).unwrap_or_else(|_| scalar_str(s)),
        _ => scalar_str(s),
    }
}

fn scalar_text(src: &str, old: &Node, v: &Value) -> String {
    match v {
        Value::String(s) if old.quoted => quoted_like(src, old, s),
        other => inline(other),
    }
}

fn diff_value(
    src: &str,
    entry: &tree::Entry,
    want: &Value,
    path: &[String],
    depth: usize,
    unit: usize,
    edits: &mut Edits,
) {
    let old = &entry.value;
    let key_line = line_start(src, entry.key_span.start.offset);
    let key_indent = entry.key_span.start.offset - key_line;
    let child_indent = " ".repeat(key_indent + unit);
    let block_map =
        matches!(&old.value, tree::Value::Map(m) if !m.is_empty()) && !is_flow(src, old);
    let block_seq =
        matches!(&old.value, tree::Value::Seq(s) if !s.is_empty()) && !is_flow(src, old);
    let scalar_old = !matches!(old.value, tree::Value::Map(_) | tree::Value::Seq(_));
    let key_end = entry.key_span.end.offset.min(src.len());
    let colon_here = src[key_end..line_end(src, key_end).max(key_end)]
        .trim_start_matches([' ', '"', '\''])
        .starts_with(':');
    if !colon_here || old.span.start.offset < key_end && !matches!(old.value, tree::Value::Null) {
        edits.give_up(path, "a key written in an unusual way");
        return;
    }
    let after_colon = || {
        let from = entry.key_span.end.offset;
        from + src[from..].find(':').unwrap_or(0) + 1
    };
    let value_end = |edits: &mut Edits| -> Option<usize> {
        let end = if is_flow(src, old) {
            flow_end(src, old.span.start.offset)
        } else {
            content_end(old).max(entry.key_span.end.offset)
        };
        let line = line_end(src, end);
        if after_colon() > line {
            edits.give_up(path, "a key written in an unusual way");
            return None;
        }
        let own = src[after_colon()..line].trim_end_matches('\n');
        if own.contains('\n') && scalar_old {
            edits.give_up(path, "a multi-line scalar");
            return None;
        }
        Some(end)
    };
    match want {
        Value::Object(m) if block_map && !m.is_empty() => {
            diff_map(src, old, want, path, depth, unit, edits);
        }
        Value::Array(items) if block_seq && !items.is_empty() => {
            replace_block_seq(src, old, items, path, edits);
        }
        Value::Object(m) if !m.is_empty() => {
            let Some(end) = value_end(edits) else {
                return;
            };
            let flow = crate::config::emit::flow(want);
            let comment = {
                let le = line_end(src, end);
                let line = src[key_line..le].trim_end_matches('\n');
                comment_at(line)
                    .filter(|at| key_line + at >= end)
                    .map(|at| line[at..].to_string())
            };
            if old.span.start.line == old.span.end.line
                && key_indent + entry.key.len() + 2 + flow.len() <= 100
                && (scalar_old || is_flow(src, old))
            {
                let (from, with) = inline_from(src, entry, flow);
                edits.push(from, end, with);
                return;
            }
            let body: String = render(want, Format::Yaml)
                .lines()
                .map(|l| format!("{child_indent}{l}\n"))
                .collect();
            let tail_end = line_end(src, end);
            let head = match comment {
                Some(c) => format!(" {c}\n"),
                None => "\n".to_string(),
            };
            edits.push(after_colon(), tail_end, format!("{head}{body}"));
        }
        Value::Array(a) if !a.is_empty() => {
            let Some(end) = value_end(edits) else {
                return;
            };
            if a.iter().any(|v| v.is_object()) {
                edits.give_up(path, "a list of mappings");
                return;
            }
            if is_flow(src, old)
                && let tree::Value::Seq(old_items) = &old.value
                && let Some(text) = flow_subset(src, old, old_items, a, end)
            {
                edits.push(old.span.start.offset, end, text);
                return;
            }
            let flow = crate::config::emit::flow(want);
            let single = old.span.start.line == old.span.end.line || scalar_old;
            if single && key_indent + entry.key.len() + 2 + flow.len() <= 100 {
                let (from, with) = inline_from(src, entry, flow);
                edits.push(from, end, with);
                return;
            }
            let comment = {
                let le = line_end(src, end);
                let line = src[key_line..le].trim_end_matches('\n');
                comment_at(line)
                    .filter(|at| key_line + at >= end)
                    .map(|at| line[at..].to_string())
            };
            let body: String = a
                .iter()
                .map(|v| format!("{child_indent}- {}\n", crate::config::emit::flow(v)))
                .collect();
            let head = match comment {
                Some(c) => format!(" {c}\n"),
                None => "\n".to_string(),
            };
            edits.push(after_colon(), line_end(src, end), format!("{head}{body}"));
        }
        scalar => {
            if block_map || block_seq {
                let end = end_of(src, key_line, content_end(old), key_indent);
                let text = format!(
                    "{}: {}\n",
                    &src[key_line..entry.key_span.end.offset],
                    scalar_text(src, old, scalar)
                );
                let comment = src[key_line..line_end(src, entry.key_span.end.offset)]
                    .trim_end_matches('\n')
                    .to_string();
                let c = comment_at(&comment).map(|at| comment[at..].to_string());
                let text = match c {
                    Some(c) => format!("{} {c}\n", text.trim_end_matches('\n')),
                    None => text,
                };
                edits.push(key_line, end, text);
                return;
            }
            let Some(end) = value_end(edits) else {
                return;
            };
            let (from, with) = inline_from(src, entry, scalar_text(src, old, scalar));
            edits.push(from, end, with);
        }
    }
}

fn replace_block_seq(src: &str, old: &Node, items: &[Value], path: &[String], edits: &mut Edits) {
    let tree::Value::Seq(old_items) = &old.value else {
        return;
    };
    let first = line_start(src, old_items[0].span.start.offset);
    let dash = src[first..].find('-').map(|i| first + i);
    let item_indent = dash.and_then(|d| src.get(first..d)).unwrap_or("-");
    if item_indent.chars().any(|c| c != ' ')
        || dash.is_none_or(|d| d > old_items[0].span.start.offset)
    {
        edits.give_up(path, "the list items do not start their lines");
        return;
    }
    let last = &old_items[old_items.len() - 1];
    let end = end_of(src, first, last.span.end.offset, item_indent.len());
    let mut lines = Vec::new();
    for it in old_items {
        let ls = line_start(src, it.span.start.offset);
        let le = line_end(src, content_end(it));
        let own = src[ls..le].starts_with(&format!("{item_indent}-"));
        lines.push((it.to_json(), own.then(|| src[ls..le].to_string())));
    }
    let mut text = String::new();
    for v in items {
        if matches!(v, Value::Object(o) if !o.is_empty()) {
            edits.give_up(path, "a list of mappings");
            return;
        }
        match lines
            .iter()
            .position(|(old, line)| old == v && line.is_some())
        {
            Some(pos) => {
                let line = lines.remove(pos).1.unwrap_or_default();
                text.push_str(&line);
                if !line.ends_with('\n') {
                    text.push('\n');
                }
            }
            None => {
                text.push_str(item_indent);
                text.push_str("- ");
                text.push_str(&crate::config::emit::flow(v));
                text.push('\n');
            }
        }
    }
    edits.push(first, end, text);
}

fn flow_subset(
    src: &str,
    old: &Node,
    old_items: &[Node],
    want: &[Value],
    end: usize,
) -> Option<String> {
    let first = old_items.first()?;
    let last = old_items.last()?;
    let sep = match old_items.get(1) {
        Some(second) => src.get(first.span.end.offset..second.span.start.offset)?,
        None => ", ",
    };
    if sep.contains('#') || !src.get(old.span.start.offset..)?.starts_with('[') {
        return None;
    }
    let open = src.get(old.span.start.offset + 1..first.span.start.offset)?;
    let close = src.get(last.span.end.offset..end.checked_sub(1)?)?;
    let mut pool: Vec<(Value, &str)> = old_items
        .iter()
        .map(|it| {
            Some((
                it.to_json(),
                src.get(it.span.start.offset..it.span.end.offset)?,
            ))
        })
        .collect::<Option<_>>()?;
    let mut kept = Vec::new();
    for v in want {
        let at = pool.iter().position(|(j, _)| j == v)?;
        kept.push(pool.remove(at).1);
    }
    Some(format!("[{open}{}{close}]", kept.join(sep)))
}

fn content_end(node: &Node) -> usize {
    match &node.value {
        tree::Value::Seq(items) => items.last().map_or(node.span.end.offset, content_end),
        tree::Value::Map(entries) => entries.last().map_or(node.span.end.offset, |e| {
            if matches!(e.value.value, tree::Value::Null) {
                e.key_span
                    .end
                    .offset
                    .max(e.value.span.end.offset.min(e.key_span.end.offset + 1))
            } else {
                content_end(&e.value)
            }
        }),
        _ => node.span.end.offset,
    }
}

fn flow_end(src: &str, start: usize) -> usize {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (i, c) in src[start..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return start + i + 1;
                }
            }
            _ => {}
        }
    }
    src.len()
}

fn inline_from(src: &str, entry: &tree::Entry, text: String) -> (usize, String) {
    let old = &entry.value;
    let colon = {
        let from = entry.key_span.end.offset;
        from + src[from..].find(':').unwrap_or(0) + 1
    };
    let empty = matches!(old.value, tree::Value::Null)
        && src[old.span.start.offset.min(src.len())..old.span.end.offset.min(src.len())]
            .trim()
            .is_empty();
    if empty || old.span.start.offset < colon {
        (colon, format!(" {text}"))
    } else {
        (old.span.start.offset, text)
    }
}
