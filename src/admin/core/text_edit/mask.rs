use std::collections::{BTreeMap, HashMap};

use blake2::digest::{KeyInit, Mac};
use burrow_admin_proto::schema::Node as SchemaNode;
use burrow_admin_proto::text_edit::placeholder;

use super::spots::{Place, Spot, comment_spots, field_spots};
use crate::config::Format;
use crate::config::patch::Rename;
use crate::config::tree::{self, Node};
use crate::wire::secret::Secret;

pub struct Ids {
    key: [u8; 32],
    #[cfg(test)]
    pub same_fingerprint: bool,
}

impl Default for Ids {
    fn default() -> Ids {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).expect("operating system random source is available");
        Ids {
            key,
            #[cfg(test)]
            same_fingerprint: false,
        }
    }
}

pub type Taken = HashMap<(Place, String), String>;

impl Ids {
    pub fn of(&self, place: &Place, value: &str) -> String {
        if let Ok(s) = Secret::parse(value) {
            #[cfg(test)]
            if self.same_fingerprint {
                return "00000000".into();
            }
            return s.fingerprint();
        }
        self.mac(place, value)
    }

    pub fn free(&self, place: &Place, value: &str, taken: &mut Taken) -> String {
        let mut id = self.of(place, value);
        let mut n = 0u32;
        while taken
            .get(&(place.clone(), id.clone()))
            .is_some_and(|v| v != value)
        {
            n += 1;
            id = self.mac(place, &format!("{value}\u{0}{n}"));
        }
        taken.insert((place.clone(), id.clone()), value.to_string());
        id
    }

    fn mac(&self, place: &Place, value: &str) -> String {
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
    pub fn written_for(&self, style: Style, indent: usize, format: Format) -> String {
        let quoted = serde_json::to_string(&self.value).unwrap_or_default();
        if format == Format::Json {
            let same = match serde_json::from_str::<serde_json::Value>(&self.written) {
                Ok(serde_json::Value::String(v)) => v == self.value,
                Ok(v @ (serde_json::Value::Number(_) | serde_json::Value::Bool(_))) => {
                    serde_json::from_str::<serde_json::Value>(&self.value).is_ok_and(|x| x == v)
                }
                _ => false,
            };
            return if same { self.written.clone() } else { quoted };
        }
        let moved = self.written.contains('\n') && indent != self.indent;
        if style == self.shown && !moved {
            return self.written.clone();
        }
        quoted
    }
}

pub type Table = BTreeMap<(Place, String), Kept>;

pub struct Masked {
    pub text: String,
    pub kept: Vec<((Place, String), Kept)>,
    pub tree: Node,
    pub fields: Vec<String>,
}

pub fn indent_at(body: &str, at: usize) -> usize {
    let line = body[..at].rfind('\n').map_or(0, |i| i + 1);
    body[line..].len() - body[line..].trim_start_matches(' ').len()
}

pub fn spots(tree: &Node, body: &str, root: &SchemaNode, file: &str) -> Vec<Spot> {
    let mut all = field_spots(tree, body, root);
    all.extend(comment_spots(tree, body, file));
    all.sort_by_key(|s| s.range.start);
    all
}

pub fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(body) => ("\u{feff}", body),
        None => ("", text),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Named<'a> {
    pub name: &'a str,
    pub format: Format,
}

pub fn mask(file: Named, text: &str, root: &SchemaNode, ids: &Ids) -> Result<Masked, String> {
    mask_renamed(file, text, root, ids, &[], &mut Taken::new())
}

fn renamed(place: &Place, renames: &[Rename]) -> Place {
    let Place::Field(path) = place else {
        return place.clone();
    };
    let mut path = path.clone();
    for r in renames {
        if let Some((_, parent)) = r.path.split_last()
            && path.len() >= r.path.len()
            && path.starts_with(&r.path)
        {
            path[parent.len()] = r.to.clone();
        }
    }
    Place::Field(path)
}

pub fn mask_renamed(
    file: Named,
    text: &str,
    root: &SchemaNode,
    ids: &Ids,
    renames: &[Rename],
    taken: &mut Taken,
) -> Result<Masked, String> {
    let json = file.format == Format::Json;
    let (bom, body) = split_bom(text);
    let tree = tree::parse(body).map_err(|e| {
        e.first()
            .map(|e| format!("line {}: {}", e.span.start.line, e.message))
            .unwrap_or_default()
    })?;
    let mut out = String::with_capacity(text.len());
    out.push_str(bom);
    let mut kept = Vec::new();
    let mut fields = Vec::new();
    let mut at = 0;
    for s in spots(&tree, body, root, file.name) {
        if matches!(s.place, Place::Field(_)) {
            fields.push(s.value.clone());
        }
        if s.range.start < at || s.range.is_empty() {
            continue;
        }
        let written = &body[s.range.clone()];
        let id = ids.free(&renamed(&s.place, renames), &s.value, taken);
        let token = match s.place {
            Place::Comment(_) => placeholder(&id),
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
    Ok(Masked {
        text: out,
        kept,
        tree,
        fields,
    })
}
