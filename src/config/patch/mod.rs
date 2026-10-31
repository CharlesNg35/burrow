mod diff;
mod yaml;

use serde_json::{Map, Value};

use super::Format;
use super::emit::render;
use super::tree;
use yaml::{child, content_end, entry_bounds, is_flow, patch_yaml};

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

pub fn split_text(text: &str) -> (&'static str, &'static str, String) {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    if body.contains("\r\n") {
        return (bom, "\r\n", body.replace("\r\n", "\n"));
    }
    if body.contains('\r') {
        return (bom, "\r", body.replace('\r', "\n"));
    }
    (bom, "\n", body.to_string())
}

fn join_text(bom: &str, eol: &str, out: String) -> String {
    let out = if eol == "\n" {
        out
    } else {
        out.replace('\n', eol)
    };
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
        Format::Yaml => {
            let out = patch_yaml(text, target, renames)?;
            let back = tree::parse(&split_text(&out.text).2).map(|n| n.to_json());
            if back.as_ref() != Ok(target) {
                return Err(PatchError::Verify(
                    "the written file would not read back as the wanted values".into(),
                ));
            }
            Ok(out)
        }
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
    let (bom, eol, src) = split_text(text);
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
        text: join_text(bom, eol, out),
        reformat: None,
    })
}
#[cfg(test)]
mod tests;
