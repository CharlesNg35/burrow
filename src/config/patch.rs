use serde_json::{Map, Value};

use super::Format;
use super::emit::{inline, render, scalar_str};
use super::tree::{self, Node};

pub fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

pub fn line_end(text: &str, at: usize) -> usize {
    if at == 0 || text[..at].ends_with('\n') {
        return at;
    }
    text[at..].find('\n').map_or(text.len(), |i| at + i + 1)
}

pub fn own_lines(block: &str, indent: usize) -> usize {
    let mut keep = block.len();
    for line in block.split_inclusive('\n').rev() {
        let body = line.trim_start();
        let comment = body.starts_with('#') && line.len() - body.len() <= indent;
        if line.trim_end().is_empty() || comment {
            keep -= line.len();
        } else {
            break;
        }
    }
    keep
}

pub fn end_of(src: &str, start: usize, node_end: usize, indent: usize) -> usize {
    let end = line_end(src, node_end.max(start));
    start + own_lines(&src[start..end], indent)
}

pub fn previous_line(src: &str, at: usize) -> Option<(usize, &str)> {
    if at == 0 {
        return None;
    }
    let start = line_start(src, at - 1);
    Some((start, &src[start..at]))
}

pub fn lead(src: &str, start: usize, indent: &str) -> usize {
    let mut at = start;
    while let Some((before, line)) = previous_line(src, at) {
        match line.strip_prefix(indent) {
            Some(rest) if rest.starts_with('#') => at = before,
            _ => break,
        }
    }
    at
}

pub fn comment_at(line: &str) -> Option<usize> {
    let mut quote = None;
    let mut before = ' ';
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '#') if before.is_whitespace() => return Some(i),
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            _ => {}
        }
        before = c;
    }
    None
}

pub fn trailing_comment(block: &str) -> Option<&str> {
    block
        .lines()
        .rev()
        .find(|line| {
            let content = comment_at(line).map_or(*line, |at| &line[..at]);
            !content.trim().is_empty()
        })
        .and_then(|line| comment_at(line).map(|at| line[at..].trim_end()))
}

pub fn entry_yaml(key: &str, value: &Value, indent: &str) -> String {
    let mut out = String::new();
    let mut one = Map::new();
    one.insert(key.to_string(), value.clone());
    for line in render(&Value::Object(one), Format::Yaml).lines() {
        out.push_str(indent);
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    Parse(String),
    Verify(String),
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::Parse(m) => write!(f, "the config file does not parse: {m}"),
            PatchError::Verify(m) => write!(f, "patch-verify: {m}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reformat {
    pub scope: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patched {
    pub text: String,
    pub reformat: Option<Reformat>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub path: Vec<String>,
    pub to: String,
}

struct Edits {
    list: Vec<(usize, usize, String)>,
    unsupported: Option<(Vec<String>, String)>,
}

impl Edits {
    fn push(&mut self, from: usize, to: usize, with: String) {
        self.list.push((from, to, with));
    }

    fn give_up(&mut self, path: &[String], why: &str) {
        if self.unsupported.is_none() {
            self.unsupported = Some((path.to_vec(), why.to_string()));
        }
    }
}

pub fn split_text(text: &str) -> (&'static str, bool, String) {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let crlf = body.contains("\r\n");
    let src = if crlf {
        body.replace("\r\n", "\n")
    } else {
        body.to_string()
    };
    (bom, crlf, src)
}

fn join_text(bom: &str, crlf: bool, out: String) -> String {
    let out = if crlf { out.replace('\n', "\r\n") } else { out };
    format!("{bom}{out}")
}

pub fn patch(
    text: &str,
    format: Format,
    target: &Value,
    renames: &[Rename],
) -> Result<Patched, PatchError> {
    match format {
        Format::Json => patch_json(text, target),
        Format::Yaml => patch_yaml(text, target, renames),
    }
}

fn json_indent(src: &str) -> usize {
    src.lines()
        .skip(1)
        .find_map(|l| {
            let n = l.len() - l.trim_start_matches(' ').len();
            (n > 0).then_some(n)
        })
        .unwrap_or(2)
}

fn patch_json(text: &str, target: &Value) -> Result<Patched, PatchError> {
    let (bom, crlf, src) = split_text(text);
    let old: Value = serde_json::from_str(&src).map_err(|e| PatchError::Parse(e.to_string()))?;
    if &old == target {
        return Ok(Patched {
            text: text.to_string(),
            reformat: None,
        });
    }
    let indent = " ".repeat(json_indent(&src));
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    serde::Serialize::serialize(target, &mut ser).map_err(|e| PatchError::Verify(e.to_string()))?;
    let mut out = String::from_utf8(buf).map_err(|e| PatchError::Verify(e.to_string()))?;
    if src.ends_with('\n') || src.is_empty() {
        out.push('\n');
    }
    let back: Value = serde_json::from_str(&out).map_err(|e| PatchError::Verify(e.to_string()))?;
    if &back != target {
        return Err(PatchError::Verify(
            "the JSON result differs from the wanted values".into(),
        ));
    }
    Ok(Patched {
        text: join_text(bom, crlf, out),
        reformat: None,
    })
}

fn patch_yaml(text: &str, target: &Value, renames: &[Rename]) -> Result<Patched, PatchError> {
    let (bom, crlf, src) = split_text(text);
    let mut root = tree::parse(&src)
        .map_err(|e| PatchError::Parse(e.first().map(|e| e.message.clone()).unwrap_or_default()))?;
    if &root.to_json() == target && renames.is_empty() {
        return Ok(Patched {
            text: text.to_string(),
            reformat: None,
        });
    }
    let unit = indent_unit(&src);
    let mut edits = Edits {
        list: Vec::new(),
        unsupported: None,
    };
    for r in renames {
        rename_key(&src, &mut root, r, &mut edits);
    }
    let is_doc_map = matches!(root.value, tree::Value::Map(ref m) if !m.is_empty())
        && !src[root.span.start.offset..].starts_with('{');
    if is_doc_map && target.is_object() {
        diff_map(&src, &root, target, &[], 0, unit, &mut edits);
    } else {
        edits.give_up(&[], "the file is not a block mapping");
    }
    let attempt = if edits.unsupported.is_none() {
        apply(&src, &edits.list).filter(|out| verify(out, target))
    } else {
        None
    };
    if let Some(out) = attempt {
        return Ok(Patched {
            text: join_text(bom, crlf, out),
            reformat: None,
        });
    }
    let reason = edits
        .unsupported
        .clone()
        .map(|(_, r)| r)
        .unwrap_or_else(|| {
            "this part of the file has a layout the patcher cannot edit in place".into()
        });
    let scope = edits
        .unsupported
        .as_ref()
        .and_then(|(p, _)| p.first().cloned())
        .map(|k| vec![k]);
    if let Some(scope) = scope
        && is_doc_map
        && let Some(out) = reformat_section(&src, &root, target, &scope[0], renames)
        && verify(&out, target)
    {
        return Ok(Patched {
            text: join_text(bom, crlf, out),
            reformat: Some(Reformat { scope, reason }),
        });
    }
    let out = render(target, Format::Yaml);
    if !verify(&out, target) {
        return Err(PatchError::Verify(
            "the rewritten file does not match the wanted values".into(),
        ));
    }
    Ok(Patched {
        text: join_text(bom, crlf, out),
        reformat: Some(Reformat {
            scope: Vec::new(),
            reason,
        }),
    })
}

fn verify(out: &str, target: &Value) -> bool {
    tree::parse(out).is_ok_and(|n| &n.to_json() == target) && !out.contains('\t')
}

fn apply(src: &str, edits: &[(usize, usize, String)]) -> Option<String> {
    let mut sorted: Vec<&(usize, usize, String)> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.0, e.1));
    for w in sorted.windows(2) {
        if w[0].1 > w[1].0 {
            return None;
        }
    }
    let mut out = src.to_string();
    for (from, to, with) in sorted.into_iter().rev() {
        if *to > out.len() || from > to {
            return None;
        }
        out.replace_range(*from..*to, with);
    }
    Some(out)
}

fn reformat_section(
    src: &str,
    root: &Node,
    target: &Value,
    key: &str,
    renames: &[Rename],
) -> Option<String> {
    let tree::Value::Map(entries) = &root.value else {
        return None;
    };
    let wanted = target.as_object()?;
    let renamed = renames
        .iter()
        .find(|r| r.path.len() == 1 && r.path[0] == key)
        .map_or(key, |r| r.to.as_str());
    let idx = entries.iter().position(|e| e.key == key)?;
    let start = line_start(src, entries[idx].key_span.start.offset);
    let end = match entries.get(idx + 1) {
        Some(next) => {
            let ns = line_start(src, next.key_span.start.offset);
            start + own_lines(&src[start..lead(src, ns, "")], 0)
        }
        None => end_of(src, start, entries[idx].value.span.end.offset, 0),
    };
    let text = match wanted.get(renamed) {
        Some(v) => entry_yaml(renamed, v, ""),
        None => String::new(),
    };
    let mut out = String::new();
    out.push_str(&src[..start]);
    out.push_str(&text);
    out.push_str(&src[end..]);
    let mut rest = Map::new();
    for (k, v) in wanted {
        let known = entries.iter().any(|e| e.key == *k) || k == renamed;
        if !known {
            rest.insert(k.clone(), v.clone());
        }
    }
    if !rest.is_empty() {
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&render(&Value::Object(rest), Format::Yaml));
    }
    Some(out)
}

fn indent_unit(src: &str) -> usize {
    src.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| l.len() - l.trim_start_matches(' ').len())
        .filter(|n| *n > 0)
        .min()
        .unwrap_or(2)
        .clamp(1, 8)
}

fn rename_key(src: &str, root: &mut Node, r: &Rename, edits: &mut Edits) {
    let Some((last, parents)) = r.path.split_last() else {
        return;
    };
    let mut node = &mut *root;
    for p in parents {
        let tree::Value::Map(entries) = &mut node.value else {
            return;
        };
        let Some(e) = entries.iter_mut().find(|e| &e.key == p) else {
            return;
        };
        node = &mut e.value;
    }
    let tree::Value::Map(entries) = &mut node.value else {
        return;
    };
    let Some(e) = entries.iter_mut().find(|e| &e.key == last) else {
        return;
    };
    let span = e.key_span;
    if src[span.start.offset..span.end.offset].contains('\n') {
        edits.give_up(&r.path, "the key spans several lines");
        return;
    }
    edits.push(span.start.offset, span.end.offset, scalar_str(&r.to));
    e.key = r.to.clone();
}

fn is_flow(src: &str, node: &Node) -> bool {
    src[node.span.start.offset..].starts_with(['{', '['])
}

fn child(path: &[String], key: &str) -> Vec<String> {
    let mut p = path.to_vec();
    p.push(key.to_string());
    p
}

fn entry_bounds(src: &str, entries: &[tree::Entry], indent: &str) -> Vec<(usize, usize)> {
    let starts: Vec<usize> = entries
        .iter()
        .map(|e| line_start(src, e.key_span.start.offset))
        .collect();
    (0..entries.len())
        .map(|i| {
            let end = match starts.get(i + 1) {
                Some(&next) => {
                    starts[i] + own_lines(&src[starts[i]..lead(src, next, indent)], indent.len())
                }
                None => end_of(
                    src,
                    starts[i],
                    entries[i]
                        .value
                        .span
                        .end
                        .offset
                        .max(entries[i].key_span.end.offset),
                    indent.len(),
                ),
            };
            (starts[i], end)
        })
        .collect()
}

fn diff_map(
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

fn flow_text(v: &Value) -> String {
    match v {
        Value::Object(m) if !m.is_empty() => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{}: {}", scalar_str(k), flow_text(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Value::Array(a) if !a.is_empty() => {
            let parts: Vec<String> = a.iter().map(flow_text).collect();
            format!("[{}]", parts.join(", "))
        }
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
    let after_colon = || {
        let from = entry.key_span.end.offset;
        from + src[from..].find(':').unwrap_or(0) + 1
    };
    let value_end = |edits: &mut Edits| -> Option<usize> {
        let end = if is_flow(src, old) {
            flow_end(src, old.span.start.offset)
        } else {
            old.span.end.offset.max(entry.key_span.end.offset)
        };
        let line = line_end(src, end);
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
            let tree::Value::Seq(old_items) = &old.value else {
                return;
            };
            let first = line_start(src, old_items[0].span.start.offset);
            let dash = src[first..].find('-').map_or(0, |i| first + i);
            let item_indent = &src[first..dash];
            if item_indent.chars().any(|c| c != ' ') {
                edits.give_up(path, "the list items do not start their lines");
                return;
            }
            let last = &old_items[old_items.len() - 1];
            let end = end_of(src, first, last.span.end.offset, item_indent.len());
            let mut comments = Vec::new();
            for it in old_items {
                let ls = line_start(src, it.span.start.offset);
                let le = line_end(src, it.span.end.offset);
                let line = src[ls..le].trim_end_matches('\n');
                let c = comment_at(line).map(|at| line[at..].to_string());
                comments.push((it.to_json(), c));
            }
            let mut text = String::new();
            for v in items {
                if matches!(v, Value::Object(o) if !o.is_empty()) {
                    edits.give_up(path, "a list of mappings");
                    return;
                }
                text.push_str(item_indent);
                text.push_str("- ");
                text.push_str(&flow_text(v));
                if let Some(pos) = comments.iter().position(|(old, _)| old == v)
                    && let Some(c) = comments.remove(pos).1
                {
                    text.push(' ');
                    text.push_str(&c);
                }
                text.push('\n');
            }
            edits.push(first, end, text);
        }
        Value::Object(m) if !m.is_empty() => {
            let Some(end) = value_end(edits) else {
                return;
            };
            let flow = flow_text(want);
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
            let flow = flow_text(want);
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
                .map(|v| format!("{child_indent}- {}\n", flow_text(v)))
                .collect();
            let head = match comment {
                Some(c) => format!(" {c}\n"),
                None => "\n".to_string(),
            };
            edits.push(after_colon(), line_end(src, end), format!("{head}{body}"));
        }
        scalar => {
            if block_map || block_seq {
                let end = end_of(src, key_line, old.span.end.offset, key_indent);
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

#[cfg(test)]
mod tests;
