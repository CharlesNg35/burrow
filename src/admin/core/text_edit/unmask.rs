use std::ops::Range;

use burrow_admin_proto::codes::SECRET_PLACEHOLDER;
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_admin_proto::text_edit::{placeholder, placeholders_in};

use super::mask::{Style, Table, indent_at, split_bom};
use super::spots::{Place, comments, field_place, scalar_range, scalar_text};
use crate::config::tree::{self, Node, Value};

#[derive(Debug, Clone)]
struct Move {
    done: Range<usize>,
    sent: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct Shift {
    pub file: String,
    sent: String,
    done: String,
    moves: Vec<Move>,
}

struct Found {
    at: Range<usize>,
    text: String,
    value: Option<String>,
}

struct Visit<'a> {
    file: &'a str,
    body: &'a str,
    table: &'a Table,
    path: Vec<String>,
    place: Vec<Option<String>>,
    found: Vec<Found>,
    problems: Vec<Diag>,
}

fn position(body: &str, at: usize) -> (u32, u32) {
    let before = &body[..at.min(body.len())];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    (line as u32, before[start..].chars().count() as u32 + 1)
}

fn offset(body: &str, line: u32, col: u32) -> Option<usize> {
    let mut start = 0;
    for _ in 1..line {
        start += body[start..].find('\n')? + 1;
    }
    let rest = &body[start..];
    let col = usize::try_from(col.saturating_sub(1)).unwrap_or(0);
    Some(start + rest.char_indices().nth(col).map_or(rest.len(), |(i, _)| i))
}

impl Visit<'_> {
    fn problem(&mut self, at: Range<usize>, message: String) {
        let (line, col) = position(self.body, at.start);
        let (end_line, end_col) = position(self.body, at.end);
        let shown = self.path.join(".");
        let message = match shown.as_str() {
            "" => format!("{} line {line}: {message}", self.file),
            _ => format!("{} line {line}, {shown}: {message}", self.file),
        };
        self.problems.push(Diag {
            severity: Severity::Error,
            code: SECRET_PLACEHOLDER.into(),
            message,
            path: (!self.path.is_empty()).then(|| self.path.clone()),
            also: Vec::new(),
            label: Some("not a secret of this field".into()),
            help: Some(
                "type the secret here, or put the placeholder back in the field it came from"
                    .into(),
            ),
            line: Some(line),
            col: Some(col),
            end_line: Some(end_line),
            end_col: Some(end_col),
            before: false,
            subjects: Vec::new(),
            file: Some(self.file.to_string()),
        });
    }

    fn walk(&mut self, n: &Node) {
        match &n.value {
            Value::Map(entries) => {
                for e in entries {
                    self.path.push(e.key.clone());
                    self.place.push(Some(e.key.clone()));
                    if !placeholders_in(&e.key).is_empty() {
                        let at = e.key_span.start.offset..e.key_span.end.offset;
                        self.problem(at, "a secret placeholder cannot be a key".into());
                    }
                    self.walk(&e.value);
                    self.path.pop();
                    self.place.pop();
                }
            }
            Value::Seq(items) => {
                for (i, it) in items.iter().enumerate() {
                    self.path.push(i.to_string());
                    self.place.push(None);
                    self.walk(it);
                    self.path.pop();
                    self.place.pop();
                }
            }
            Value::Str(s) => self.scalar(n, s),
            _ => {}
        }
    }

    fn scalar(&mut self, n: &Node, s: &str) {
        let found = placeholders_in(s);
        let Some((range, id)) = found.first() else {
            return;
        };
        let at = scalar_range(n, self.body);
        if found.len() > 1 || *range != (0..s.len()) {
            let why = "a secret placeholder stands for a whole value; type the secret instead";
            self.problem(at, why.into());
            return;
        }
        let key = (field_place(&self.place), id.to_string());
        match self.table.get(&key) {
            Some(k) => {
                let style = Style::of(&self.body[at.clone()]);
                let text = k.written_for(style, indent_at(self.body, at.start));
                self.found.push(Found {
                    at,
                    text,
                    value: Some(k.value.clone()),
                });
            }
            None => {
                let why = match self
                    .table
                    .iter()
                    .find(|((p, i), _)| i == id && matches!(p, Place::Field(_)))
                {
                    Some((_, k)) => format!(
                        "this placeholder stands for the secret of {}; type the secret or keep it in its own field",
                        k.path.join(".")
                    ),
                    None => format!(
                        "{} stands for no current secret (the server may have restarted: reload the text), or type the secret",
                        placeholder(id)
                    ),
                };
                self.problem(at, why);
            }
        }
    }

    fn comments(&mut self, tree: &Node) {
        for c in comments(tree, self.body) {
            for (r, id) in placeholders_in(&self.body[c.clone()]) {
                let at = c.start + r.start..c.start + r.end;
                match self.table.get(&(Place::Comment, id.to_string())) {
                    Some(k) => self.found.push(Found {
                        at,
                        text: k.written.clone(),
                        value: None,
                    }),
                    None => {
                        let why = format!(
                            "{} in a comment stands for no current secret; remove it",
                            placeholder(id)
                        );
                        self.problem(at, why);
                    }
                }
            }
        }
    }
}

pub fn unmask(file: &str, text: &str, table: &Table) -> Result<(String, Shift), Vec<Diag>> {
    let (bom, body) = split_bom(text);
    let mut shift = Shift {
        file: file.to_string(),
        sent: body.to_string(),
        done: body.to_string(),
        moves: Vec::new(),
    };
    let Ok(tree) = tree::parse(body) else {
        return Ok((text.to_string(), shift));
    };
    let mut v = Visit {
        file,
        body,
        table,
        path: Vec::new(),
        place: Vec::new(),
        found: Vec::new(),
        problems: Vec::new(),
    };
    v.walk(&tree);
    v.comments(&tree);
    if !v.problems.is_empty() {
        return Err(v.problems);
    }
    let mut found = v.found;
    found.sort_by_key(|f| f.at.start);
    let mut moves = build(body, &found);
    let wrong = misread(&moves.0, &moves.1, &found);
    if !wrong.is_empty() {
        for i in wrong {
            let value = found[i].value.clone().unwrap_or_default();
            found[i].text = serde_json::to_string(&value).unwrap_or_default();
        }
        moves = build(body, &found);
        if !misread(&moves.0, &moves.1, &found).is_empty() {
            return Err(vec![v_problem(file, body)]);
        }
    }
    let (done, list) = moves;
    shift.moves = list;
    shift.done = done.clone();
    Ok((format!("{bom}{done}"), shift))
}

fn build(body: &str, found: &[Found]) -> (String, Vec<Move>) {
    let mut done = String::with_capacity(body.len());
    let mut moves = Vec::new();
    let mut at = 0;
    for f in found {
        done.push_str(&body[at..f.at.start]);
        let start = done.len();
        done.push_str(&f.text);
        moves.push(Move {
            done: start..done.len(),
            sent: f.at.clone(),
        });
        at = f.at.end;
    }
    done.push_str(&body[at..]);
    (done, moves)
}

fn scalars(n: &Node, body: &str, out: &mut Vec<(Range<usize>, String)>) {
    match &n.value {
        Value::Map(entries) => entries.iter().for_each(|e| scalars(&e.value, body, out)),
        Value::Seq(items) => items.iter().for_each(|i| scalars(i, body, out)),
        _ => out.push((scalar_range(n, body), scalar_text(n))),
    }
}

fn misread(done: &str, moves: &[Move], found: &[Found]) -> Vec<usize> {
    let Ok(tree) = tree::parse(done) else {
        return (0..found.len())
            .filter(|i| found[*i].value.is_some())
            .collect();
    };
    let mut all = Vec::new();
    scalars(&tree, done, &mut all);
    let reads = |m: &Move, value: &str| all.iter().any(|(r, s)| *r == m.done && s == value);
    found
        .iter()
        .zip(moves)
        .enumerate()
        .filter(|(_, (f, m))| f.value.as_deref().is_some_and(|v| !reads(m, v)))
        .map(|(i, _)| i)
        .collect()
}

fn v_problem(file: &str, body: &str) -> Diag {
    let mut v = Visit {
        file,
        body,
        table: &Table::new(),
        path: Vec::new(),
        place: Vec::new(),
        found: Vec::new(),
        problems: Vec::new(),
    };
    v.problem(
        0..0,
        "a secret could not be put back where its placeholder stands; write the value on its own line".into(),
    );
    v.problems.remove(0)
}

impl Shift {
    fn back(&self, at: usize) -> usize {
        let mut delta: isize = 0;
        for m in &self.moves {
            if at >= m.done.end {
                delta += m.sent.len() as isize - m.done.len() as isize;
            } else if at >= m.done.start {
                return m.sent.start;
            } else {
                break;
            }
        }
        at.saturating_add_signed(delta)
    }

    fn moved(&self, line: u32, col: u32) -> Option<(u32, u32)> {
        let at = offset(&self.done, line, col)?;
        Some(position(&self.sent, self.back(at)))
    }

    pub fn place(&self, d: &mut Diag) {
        if self.moves.is_empty() {
            return;
        }
        if let (Some(line), Some(col)) = (d.line, d.col)
            && let Some((l, c)) = self.moved(line, col)
        {
            (d.line, d.col) = (Some(l), Some(c));
        }
        if let (Some(line), Some(col)) = (d.end_line, d.end_col)
            && let Some((l, c)) = self.moved(line, col)
        {
            (d.end_line, d.end_col) = (Some(l), Some(c));
        }
    }
}
