//! The raw config editor's side of the admin core: each config file's text with its secrets masked, and edited texts back.
//! Start at `config_text` for the files and `edited` for a `replace_text` op; `schema` builds the editor's JSON Schema.
//! A placeholder is unmasked only where the current text had it, so it can never carry a secret to another field.

mod mask;
pub mod schema;
mod spots;
mod unmask;

use std::sync::OnceLock;

use burrow_admin_proto::diag::Diag;
use burrow_admin_proto::msg::{ErrorCode, InvalidBody};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::{Change, ChangeKind, Plan};
use burrow_admin_proto::text_edit::{EditFile, EditText, PLACEHOLDER_PATTERN, TextFile};
use serde_json::Value;

use super::{ApiError, Core, plan, text, values};
use crate::config::Format;
use crate::config::docs::Docs;
pub use mask::Ids;

pub struct TextEdit {
    ids: Ids,
    schema: OnceLock<Value>,
}

impl TextEdit {
    pub fn new() -> TextEdit {
        TextEdit {
            ids: Ids::default(),
            schema: OnceLock::new(),
        }
    }
}

impl Default for TextEdit {
    fn default() -> Self {
        TextEdit::new()
    }
}

fn json_at(core: &Core, i: usize) -> bool {
    i == 0 && core.store.format() == Format::Json
}

fn masked(core: &Core, docs: &Docs) -> Result<Vec<mask::Masked>, ApiError> {
    let root = &core.schema().root;
    docs.list
        .iter()
        .enumerate()
        .map(|(i, d)| {
            mask::mask(&d.text, json_at(core, i), root, &core.text_edit.ids).map_err(|why| {
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
    })
}

const IDS: [&str; 5] = ["plan", "base", "result_rev", "current", "rev"];

fn veil_strings(v: &mut Value, hidden: &values::Hidden) {
    match v {
        Value::String(s) => *s = text::mask_secrets(&hidden.line(s), &[]),
        Value::Array(a) => a.iter_mut().for_each(|x| veil_strings(x, hidden)),
        Value::Object(m) => m
            .iter_mut()
            .filter(|(k, _)| !IDS.contains(&k.as_str()))
            .for_each(|(_, x)| veil_strings(x, hidden)),
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

fn known(old: &Docs, files: &[TextFile]) -> Result<(), ApiError> {
    for (i, f) in files.iter().enumerate() {
        if old.get(&f.name).is_none() {
            return Err(ApiError::new(
                ErrorCode::BadRequest,
                format!(
                    "{} is not one of the config files; add or remove files in Settings, Files",
                    f.name
                ),
            ));
        }
        if files[..i].iter().any(|g| g.name == f.name) {
            return Err(ApiError::new(
                ErrorCode::BadRequest,
                format!("{} is sent twice", f.name),
            ));
        }
    }
    Ok(())
}

pub fn edited(core: &Core, old: &Docs, ops: &[Op]) -> Result<Option<Edited>, ApiError> {
    let Some(files) = sent(ops)? else {
        return Ok(None);
    };
    known(old, files)?;
    let table: mask::Table = masked(core, old)?
        .into_iter()
        .flat_map(|m| m.kept)
        .rev()
        .collect();
    let mut docs = old.clone();
    let mut shifts = Vec::new();
    let mut problems = Vec::new();
    for f in files {
        match unmask::unmask(&f.name, &f.text, &table) {
            Ok((text, shift)) => {
                if let Some(d) = docs.list.iter_mut().find(|d| d.name == f.name) {
                    d.text = text;
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
    Ok(Some(Edited {
        docs,
        old: old.clone(),
        shifts,
    }))
}

impl Edited {
    fn hidden(&self, core: &Core) -> values::Hidden {
        let texts: Vec<&str> = self
            .old
            .list
            .iter()
            .chain(&self.docs.list)
            .map(|d| d.text.as_str())
            .collect();
        values::Hidden::in_texts(&texts, &core.schema().root)
    }

    pub fn veil(&self, core: &Core, plan: &mut Plan) {
        let hidden = self.hidden(core);
        if let Ok(mut v) = serde_json::to_value(&*plan) {
            veil_strings(&mut v, &hidden);
            if let Ok(back) = serde_json::from_value(v) {
                *plan = back;
            }
        }
    }

    pub fn veil_error(&self, core: &Core, mut e: ApiError) -> ApiError {
        let hidden = self.hidden(core);
        e.message = text::mask_secrets(&hidden.line(&e.message), &[]);
        veil_strings(&mut e.body, &hidden);
        e
    }

    pub fn note(&self, old: &Docs, changes: &mut Vec<Change>) {
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
