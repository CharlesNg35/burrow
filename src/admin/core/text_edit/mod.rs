//! The raw config editor's side of the admin core: each config file's text with its secrets masked, and edited texts back.
//! Start at `config_text` for the files and `edited` for a `replace_text` op, which may also make new drop-ins;
//! `schema` builds the editor's JSON Schemas, one for the main file and one for drop-ins; `search` finds text in the masked files.
//! A placeholder is unmasked only where the current text had it, so it can never carry a secret to another field.

mod mask;
pub mod schema;
mod search;
mod shown;
mod spots;
mod unmask;

use std::sync::OnceLock;

use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::msg::{ErrorCode, InvalidBody};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{Change, ChangeKind};
use burrow_admin_proto::text_edit::{EditFile, EditText, PLACEHOLDER_PATTERN, TextFile};
use serde_json::Value;

use super::{ApiError, Core, plan, text, values};
use crate::config::Format;
use crate::config::docs::Docs;
pub use mask::Ids;
pub use search::search_text;
pub use shown::Shown;

pub struct TextEdit {
    ids: Ids,
    schema: OnceLock<Value>,
    drop_in: OnceLock<Value>,
}

impl TextEdit {
    pub fn new() -> TextEdit {
        TextEdit {
            ids: Ids::default(),
            schema: OnceLock::new(),
            drop_in: OnceLock::new(),
        }
    }
}

impl Default for TextEdit {
    fn default() -> Self {
        TextEdit::new()
    }
}

fn named<'a>(core: &Core, docs: &Docs, name: &'a str) -> mask::Named<'a> {
    let main = docs.main().name == name;
    mask::Named {
        name,
        format: if main {
            core.store.format()
        } else {
            Format::Yaml
        },
    }
}

fn masked(core: &Core, docs: &Docs) -> Result<Vec<mask::Masked>, ApiError> {
    let root = &core.schema().root;
    docs.list
        .iter()
        .map(|d| {
            mask::mask(
                named(core, docs, &d.name),
                &d.text,
                root,
                &core.text_edit.ids,
            )
            .map_err(|why| {
                ApiError::new(
                    ErrorCode::Invalid,
                    format!("{} does not parse ({why}); fix it by hand first", d.name),
                )
            })
        })
        .collect()
}

pub fn hidden_values(
    tree: &crate::config::tree::Node,
    body: &str,
    root: &burrow_admin_proto::schema::Node,
) -> Vec<String> {
    spots::field_spots(tree, body, root)
        .into_iter()
        .map(|s| s.value)
        .collect()
}

pub fn editor_schema(core: &Core) -> &Value {
    core.text_edit.schema.get_or_init(|| {
        schema::editor_schema(
            crate::config::load::schema_json(crate::config::Role::Server),
            core.schema(),
        )
    })
}

pub fn drop_in_schema(core: &Core) -> &Value {
    core.text_edit
        .drop_in
        .get_or_init(|| schema::drop_in_schema(editor_schema(core)))
}

pub fn config_text(core: &Core) -> Result<EditText, ApiError> {
    let docs = core.docs()?;
    let files = docs
        .list
        .iter()
        .zip(masked(core, &docs)?)
        .map(|(d, m)| EditFile {
            name: d.name.clone(),
            text: m.text,
            read_only: core.store.read_only(&d.name),
        })
        .collect();
    Ok(EditText {
        rev: text::docs_rev(&docs),
        schema_hash: core.schema().hash.clone(),
        schema: editor_schema(core).clone(),
        placeholder: PLACEHOLDER_PATTERN.into(),
        files,
        drop_in_schema: Some(drop_in_schema(core).clone()),
    })
}

const FREE_TEXT: [&str; 6] = ["message", "label", "help", "summary", "hint", "unified"];

fn veil_text(v: &mut Value, hidden: &values::Hidden) {
    match v {
        Value::String(s) => *s = hidden.text(s),
        Value::Array(a) => a.iter_mut().for_each(|x| veil_text(x, hidden)),
        _ => {}
    }
}

fn veil_body(v: &mut Value, hidden: &values::Hidden) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(|x| veil_body(x, hidden)),
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                match FREE_TEXT.contains(&k.as_str()) {
                    true => veil_text(x, hidden),
                    false => veil_body(x, hidden),
                }
            }
        }
        _ => {}
    }
}

pub struct Edited {
    pub docs: Docs,
    old: Docs,
    shifts: Vec<unmask::Shift>,
}

fn sent(ops: &[Op]) -> Result<Option<&[TextFile]>, ApiError> {
    let Some(files) = ops.iter().find_map(|o| match o {
        Op::ReplaceText { files } => Some(files.as_slice()),
        _ => None,
    }) else {
        return Ok(None);
    };
    if ops.len() > 1 {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            "replace_text must be the only operation",
        ));
    }
    Ok(Some(files))
}

fn known(core: &Core, old: &Docs, files: &[TextFile]) -> Result<(), ApiError> {
    let prefix = crate::config::docs::drop_in_folder_name(core.store.path());
    let mut all = old.list.clone();
    for (i, f) in files.iter().enumerate() {
        if files[..i].iter().any(|g| g.name == f.name) {
            return Err(ApiError::new(
                ErrorCode::BadRequest,
                format!("{} is sent twice", f.name),
            ));
        }
        if old.get(&f.name).is_some() {
            continue;
        }
        plan::files::new_file(&all, &prefix, &f.name).map_err(|e| match e {
            plan::files::WriteError::Bad(m) => ApiError::new(ErrorCode::BadRequest, m),
            plan::files::WriteError::Patch(p) => ApiError::new(ErrorCode::Internal, p.to_string()),
        })?;
        all.push(crate::config::docs::Doc {
            name: f.name.clone(),
            text: String::new(),
        });
    }
    Ok(())
}

pub fn edited(core: &Core, old: &Docs, ops: &[Op]) -> Result<Option<Edited>, ApiError> {
    let Some(files) = sent(ops)? else {
        return Ok(None);
    };
    known(core, old, files)?;
    let table: mask::Table = masked(core, old)?
        .into_iter()
        .flat_map(|m| m.kept)
        .rev()
        .collect();
    let mut docs = old.clone();
    let mut shifts = Vec::new();
    let mut problems = Vec::new();
    for f in files {
        match unmask::unmask(named(core, old, &f.name), &f.text, &table) {
            Ok((text, shift)) => {
                match docs.list.iter_mut().find(|d| d.name == f.name) {
                    Some(d) => d.text = text,
                    None if text.trim().is_empty() => {}
                    None => docs.list.push(crate::config::docs::Doc {
                        name: f.name.clone(),
                        text,
                    }),
                }
                shifts.push(shift);
            }
            Err(found) => problems.extend(found),
        }
    }
    if let Some(first) = problems.first() {
        let more = match problems.len() {
            1 => String::new(),
            n => format!(" (and {} more)", n - 1),
        };
        return Err(
            ApiError::new(ErrorCode::Invalid, format!("{}{more}", first.message))
                .with(serde_json::to_value(InvalidBody { diags: problems }).unwrap_or_default()),
        );
    }
    docs.sort();
    Ok(Some(Edited {
        docs,
        old: old.clone(),
        shifts,
    }))
}

impl Edited {
    fn hidden(&self, core: &Core) -> values::Hidden {
        values::Hidden::of_docs(&[&self.old, &self.docs], &core.schema().root)
    }

    pub fn veil_error(&self, core: &Core, mut e: ApiError) -> ApiError {
        let hidden = self.hidden(core);
        e.message = hidden.text(&e.message);
        veil_body(&mut e.body, &hidden);
        e
    }

    pub fn note(&self, old: &Docs, changes: &mut Vec<Change>) {
        let made = self
            .docs
            .names()
            .filter(|n| old.get(n).is_none())
            .map(|name| Change {
                kind: ChangeKind::Other,
                path: Vec::new(),
                summary: format!("creates a new file, {name}"),
                part_of: None,
            });
        let made: Vec<Change> = made.collect();
        if !made.is_empty() {
            changes.splice(0..0, made);
            return;
        }
        if !changes.is_empty() {
            return;
        }
        for (name, _) in crate::config::save::changes(old, &self.docs) {
            changes.push(Change {
                kind: ChangeKind::Other,
                path: Vec::new(),
                summary: format!("{name}: comments or layout changed"),
                part_of: None,
            });
        }
    }

    pub fn place(&self, diags: &mut [Diag]) {
        let main = self.docs.main().name.clone();
        let owners = plan::files::Owners::of(&self.docs);
        for d in diags {
            if d.file.is_none() {
                let at = match d.path.as_deref() {
                    Some([section, name, ..]) if d.line.is_none() => owners.file_of(section, name),
                    _ => 0,
                };
                d.file = Some(
                    self.docs
                        .list
                        .get(at)
                        .map_or(main.clone(), |f| f.name.clone()),
                );
            }
            if let Some(s) = self
                .shifts
                .iter()
                .find(|s| d.file.as_ref() == Some(&s.file))
            {
                s.place(d);
            }
        }
    }
}

#[cfg(test)]
mod tests;
