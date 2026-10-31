use std::collections::BTreeSet;

use burrow_admin_proto::msg::ErrorCode;
use burrow_admin_proto::value::get;
use serde_json::Value;

use super::locks::renamed_from;
use crate::admin::core::ApiError;
use crate::config::patch;

fn refs_of(v: Option<&Value>) -> BTreeSet<String> {
    let Some(Value::String(s)) = v else {
        return BTreeSet::new();
    };
    let mut out: BTreeSet<String> = burrow_syntax::env::names(s)
        .into_iter()
        .map(|n| format!("${{{n}}}"))
        .collect();
    if let Some(file) = s.trim().strip_prefix(burrow_syntax::env::FILE_REF) {
        out.insert(format!("{}{}", burrow_syntax::env::FILE_REF, file.trim()));
    }
    out
}

fn walk(
    v: &Value,
    path: &mut Vec<String>,
    before: &Value,
    renames: &[patch::Rename],
) -> Option<(Vec<String>, String)> {
    match v {
        Value::Object(m) => m.iter().find_map(|(k, x)| {
            path.push(k.clone());
            let found = walk(x, path, before, renames);
            path.pop();
            found
        }),
        Value::Array(a) => a.iter().enumerate().find_map(|(i, x)| {
            path.push(i.to_string());
            let found = walk(x, path, before, renames);
            path.pop();
            found
        }),
        Value::String(_) => {
            let now = refs_of(Some(v));
            if now.is_empty() {
                return None;
            }
            let was = refs_of(get(before, &renamed_from(path, renames)));
            now.into_iter()
                .find(|r| !was.contains(r))
                .map(|r| (path.clone(), r))
        }
        _ => None,
    }
}

pub(super) fn refuse_new_references(
    (before, after): (&Value, &Value),
    renames: &[patch::Rename],
) -> Result<(), ApiError> {
    match walk(after, &mut Vec::new(), before, renames) {
        None => Ok(()),
        Some((path, reference)) => Err(ApiError::new(
            ErrorCode::Forbidden,
            format!(
                "{}: {reference} reads the server's environment or a file; the admin channel can keep or remove such references but not add one, so write it in server.yaml on the server",
                path.join(".")
            ),
        )),
    }
}
