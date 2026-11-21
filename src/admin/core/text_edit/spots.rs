use std::collections::HashSet;
use std::ops::Range;

use burrow_admin_proto::schema::Node as SchemaNode;
use burrow_admin_proto::value;
use burrow_syntax::env::{FILE_REF, is_reference};
use serde_json::Value as J;

use crate::admin::core::{text, values};
use crate::config::tree::{Node, Span, Value};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Place {
    Field(Vec<String>),
    Comment(String),
}

#[derive(Debug, Clone)]
pub struct Spot {
    pub place: Place,
    pub path: Vec<String>,
    pub range: Range<usize>,
    pub value: String,
    pub span: Span,
}

struct Walk<'a> {
    body: &'a str,
    path: Vec<String>,
    place: Vec<Option<String>>,
    out: Vec<Spot>,
}

pub fn field_place(place: &[Option<String>]) -> Place {
    Place::Field(place.iter().flatten().cloned().collect())
}

impl Place {
    pub fn key(&self) -> String {
        match self {
            Place::Field(p) => p.join("\u{0}"),
            Place::Comment(file) => format!("#{file}"),
        }
    }
}

impl Walk<'_> {
    fn enter(&mut self, key: String, item: bool) {
        self.place.push((!item).then(|| key.clone()));
        self.path.push(key);
    }

    fn leave(&mut self) {
        self.path.pop();
        self.place.pop();
    }

    fn spot(&mut self, n: &Node) {
        self.out.push(Spot {
            place: field_place(&self.place),
            path: self.path.clone(),
            range: scalar_range(n, self.body),
            value: scalar_text(n),
            span: n.span,
        });
    }

    fn shown(&mut self, n: &Node, shown: Option<&J>) {
        if shown.is_some_and(|s| value::as_secret(s).is_some() || value::is_hash(s)) {
            self.hidden(n);
            return;
        }
        match &n.value {
            Value::Map(entries) => {
                for e in entries {
                    self.enter(e.key.clone(), false);
                    self.shown(&e.value, shown.and_then(|s| s.get(&e.key)));
                    self.leave();
                }
            }
            Value::Seq(items) => {
                for (i, it) in items.iter().enumerate() {
                    self.enter(i.to_string(), true);
                    self.shown(it, shown.and_then(|s| s.get(i)));
                    self.leave();
                }
            }
            _ => {}
        }
    }

    fn hidden(&mut self, n: &Node) {
        match &n.value {
            Value::Map(entries) => {
                for e in entries {
                    self.enter(e.key.clone(), false);
                    self.hidden(&e.value);
                    self.leave();
                }
            }
            Value::Seq(items) => {
                for (i, it) in items.iter().enumerate() {
                    self.enter(i.to_string(), true);
                    self.hidden(it);
                    self.leave();
                }
            }
            Value::Null => {}
            Value::Str(s) if env_only(s) => {}
            _ => self.spot(n),
        }
    }

    fn listed(&mut self, n: &Node, wanted: &HashSet<SpanKey>) {
        match &n.value {
            Value::Map(entries) => {
                for e in entries {
                    self.enter(e.key.clone(), false);
                    self.listed(&e.value, wanted);
                    self.leave();
                }
            }
            Value::Seq(items) => {
                for (i, it) in items.iter().enumerate() {
                    self.enter(i.to_string(), true);
                    self.listed(it, wanted);
                    self.leave();
                }
            }
            Value::Null => {}
            Value::Str(s) if is_reference(s) => {}
            _ if wanted.contains(&span_key(n.span)) => self.spot(n),
            _ => {}
        }
    }
}

type SpanKey = (usize, usize, usize);

pub fn hidden_spans(tree: &Node, root: &SchemaNode) -> Vec<Span> {
    let mut out: Vec<Span> = walked(tree, "", root).into_iter().map(|s| s.span).collect();
    out.sort_by_key(|s| span_key(*s));
    out.dedup();
    out
}

pub fn covers_hidden(hidden: &[Span], at: Span) -> bool {
    let end = (at.file.index(), at.end.offset.max(at.start.offset + 1), 0);
    let before = hidden.partition_point(|h| span_key(*h) < end);
    before > 0 && {
        let h = hidden[before - 1];
        h.file == at.file && h.end.offset > at.start.offset
    }
}

fn span_key(s: Span) -> SpanKey {
    (s.file.index(), s.start.offset, s.end.offset)
}

fn env_only(s: &str) -> bool {
    is_reference(s) && !s.trim().starts_with(FILE_REF)
}

pub fn scalar_text(n: &Node) -> String {
    match &n.value {
        Value::Str(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn block_header(body: &str, start: usize) -> Option<usize> {
    let before = body.get(..start)?;
    let line = before.trim_end();
    if !before[line.len()..].contains('\n') {
        return None;
    }
    let bare = line.trim_end_matches(|c: char| matches!(c, '-' | '+') || c.is_ascii_digit());
    if line.len() - bare.len() > 2 {
        return None;
    }
    let at = bare.len().checked_sub(1)?;
    let spaced = bare[..at].ends_with([' ', '\t']) || at == 0;
    (spaced && matches!(&bare[at..], "|" | ">")).then_some(at)
}

fn quoted_end(body: &str, start: usize) -> Option<usize> {
    let rest = body.get(start..)?;
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let mut chars = rest.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            ('"', '\\') => {
                chars.next();
            }
            ('\'', '\'') if chars.peek().is_some_and(|(_, n)| *n == '\'') => {
                chars.next();
            }
            (q, c) if q == c => return Some(start + i + c.len_utf8()),
            _ => {}
        }
    }
    None
}

pub fn scalar_range(n: &Node, body: &str) -> Range<usize> {
    let mut start = n.span.start.offset.min(body.len());
    let mut end = n.span.end.offset.clamp(start, body.len());
    if matches!(n.value, Value::Str(_)) && n.quoted {
        match quoted_end(body, start) {
            Some(e) => end = e,
            None => start = block_header(body, start).unwrap_or(start),
        }
    }
    let trimmed = body[start..end].trim_end().len();
    start..start + trimmed
}

fn walked(tree: &Node, body: &str, root: &SchemaNode) -> Vec<Spot> {
    let none = |_: &str| None;
    let shown = values::present(&tree.to_json(), Some(root), "", &none);
    let mut w = Walk {
        body,
        path: Vec::new(),
        place: Vec::new(),
        out: Vec::new(),
    };
    w.shown(tree, Some(&shown));
    let extra = crate::config::check::secret_spans(tree)
        .into_iter()
        .map(span_key)
        .collect();
    w.listed(tree, &extra);
    w.out
}

pub fn field_spots(tree: &Node, body: &str, root: &SchemaNode) -> Vec<Spot> {
    let mut out = walked(tree, body, root);
    out.sort_by_key(|s| (s.range.start, s.range.end));
    out.dedup_by_key(|s| s.range.clone());
    out
}

fn covered(n: &Node, body: &str, out: &mut Vec<Range<usize>>) {
    match &n.value {
        Value::Map(entries) => {
            for e in entries {
                out.push(e.key_span.start.offset..e.key_span.end.offset);
                covered(&e.value, body, out);
            }
        }
        Value::Seq(items) => items.iter().for_each(|i| covered(i, body, out)),
        _ => out.push(scalar_range(n, body)),
    }
}

pub fn comments(tree: &Node, body: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    covered(tree, body, &mut spans);
    spans.sort_by_key(|r| r.start);
    let reach: Vec<usize> = spans
        .iter()
        .scan(0, |end, r| {
            *end = r.end.max(*end);
            Some(*end)
        })
        .collect();
    let inside = |at: usize| {
        let before = spans.partition_point(|r| r.start <= at);
        before > 0 && reach[before - 1] > at
    };
    let mut out = Vec::new();
    let mut line_start = 0;
    for line in body.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let mut prev_space = true;
        for (i, c) in content.char_indices() {
            let at = line_start + i;
            if c == '#' && prev_space && !inside(at) {
                out.push(at..line_start + content.len());
                break;
            }
            prev_space = c.is_whitespace();
        }
        line_start += line.len();
    }
    out
}

pub fn comment_spots(tree: &Node, body: &str, file: &str) -> Vec<Spot> {
    let mut out = Vec::new();
    for c in comments(tree, body) {
        let comment = &body[c.clone()];
        let urls = burrow_syntax::secret_fmt::url_passwords(comment);
        let in_url = |r: &Range<usize>| urls.iter().any(|u| u.start < r.end && r.start < u.end);
        let tokens = text::tokens(comment).into_iter();
        let hidden = tokens.filter(|r| !in_url(r) && text::hides_token(&comment[r.clone()]));
        let mut found: Vec<Range<usize>> = urls.iter().cloned().chain(hidden).collect();
        found.sort_by_key(|r| r.start);
        out.extend(found.into_iter().map(|r| Spot {
            place: Place::Comment(file.to_string()),
            path: Vec::new(),
            range: c.start + r.start..c.start + r.end,
            value: comment[r].to_string(),
            span: Span::default(),
        }));
    }
    out
}
