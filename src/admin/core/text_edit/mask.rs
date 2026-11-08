use std::collections::BTreeMap;

use blake2::digest::{KeyInit, Mac};
use burrow_admin_proto::schema::Node as SchemaNode;
use burrow_admin_proto::text_edit::placeholder;

use super::spots::{Place, Spot, comment_spots, field_spots};
use crate::config::tree::{self, Node};
use crate::wire::secret::Secret;

pub struct Ids {
    key: [u8; 32],
}

impl Default for Ids {
    fn default() -> Ids {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).expect("operating system random source is available");
        Ids { key }
    }
}

impl Ids {
    pub fn of(&self, place: &Place, value: &str) -> String {
        if let Ok(s) = Secret::parse(value) {
            return s.fingerprint();
        }
        let mut mac =
            <blake2::Blake2sMac256 as KeyInit>::new_from_slice(&self.key).expect("a 32-byte key");
        let place = place.key();
        mac.update(&(place.len() as u64).to_be_bytes());
        mac.update(place.as_bytes());
        mac.update(value.as_bytes());
        hex::encode(&mac.finalize().into_bytes()[..4])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Plain,
    Double,
    Single,
    Block,
}

impl Style {
    pub fn of(written: &str) -> Style {
        match written.as_bytes().first() {
            Some(b'"') => Style::Double,
            Some(b'\'') => Style::Single,
            Some(b'|' | b'>') => Style::Block,
            _ => Style::Plain,
        }
    }

    fn token(self, id: &str, json: bool) -> String {
        let p = placeholder(id);
        match self {
            _ if json => format!("\"{p}\""),
            Style::Double => format!("\"{p}\""),
            Style::Single => format!("'{p}'"),
            Style::Plain | Style::Block => p,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Kept {
    pub path: Vec<String>,
    pub written: String,
    pub value: String,
    pub shown: Style,
    pub indent: usize,
}

impl Kept {
    pub fn written_for(&self, style: Style, indent: usize) -> String {
        let moved = self.written.contains('\n') && indent != self.indent;
        if style == self.shown && !moved {
            return self.written.clone();
        }
        serde_json::to_string(&self.value).unwrap_or_default()
    }
}

pub type Table = BTreeMap<(Place, String), Kept>;

pub struct Masked {
    pub text: String,
    pub kept: Vec<((Place, String), Kept)>,
}

pub fn indent_at(body: &str, at: usize) -> usize {
    let line = body[..at].rfind('\n').map_or(0, |i| i + 1);
    body[line..].len() - body[line..].trim_start_matches(' ').len()
}

pub fn spots(tree: &Node, body: &str, root: &SchemaNode) -> Vec<Spot> {
    let mut all = field_spots(tree, body, root);
    all.extend(comment_spots(tree, body));
    all.sort_by_key(|s| s.range.start);
    all
}

pub fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(body) => ("\u{feff}", body),
        None => ("", text),
    }
}

pub fn mask(text: &str, json: bool, root: &SchemaNode, ids: &Ids) -> Result<Masked, String> {
    let (bom, body) = split_bom(text);
    let tree = tree::parse(body).map_err(|e| {
        e.first()
            .map(|e| format!("line {}: {}", e.span.start.line, e.message))
            .unwrap_or_default()
    })?;
    let mut out = String::with_capacity(text.len());
    out.push_str(bom);
    let mut kept = Vec::new();
    let mut at = 0;
    for s in spots(&tree, body, root) {
        if s.range.start < at || s.range.is_empty() {
            continue;
        }
        let written = &body[s.range.clone()];
        let id = ids.of(&s.place, &s.value);
        let token = match s.place {
            Place::Comment => placeholder(&id),
            Place::Field(_) => Style::of(written).token(&id, json),
        };
        out.push_str(&body[at..s.range.start]);
        out.push_str(&token);
        kept.push((
            (s.place, id),
            Kept {
                path: s.path,
                written: written.to_string(),
                value: s.value,
                shown: Style::of(&token),
                indent: indent_at(body, s.range.start),
            },
        ));
        at = s.range.end;
    }
    out.push_str(&body[at..]);
    Ok(Masked { text: out, kept })
}
