use std::path::Path;

use super::Spans;
use crate::base::failure::file_reason;
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::SecretText;

pub(super) const FILE_PREFIX: &str = "file:";

pub(super) fn read_secret(value: &str, base_dir: &Path) -> Option<SecretText> {
    let text = match value.trim().strip_prefix(FILE_PREFIX) {
        Some(path) => {
            let read = std::fs::read_to_string(base_dir.join(path.trim())).ok()?;
            zeroize::Zeroizing::new(read).trim().to_string()
        }
        None => value.trim().to_string(),
    };
    (!text.is_empty()).then(|| SecretText::new(text))
}

pub(super) struct SecretField<'a> {
    pub(super) path: Vec<String>,
    pub(super) name: String,
    pub(super) value: &'a str,
    pub(super) env: &'static str,
    pub(super) inline_ok: bool,
    pub(super) code: &'static str,
}

pub(super) fn check_secret_ref(
    f: &SecretField<'_>,
    base_dir: &Path,
    spans: &Spans,
) -> Option<Diagnostic> {
    let path: Vec<&str> = f.path.iter().map(String::as_str).collect();
    let name = &f.name;
    let at = spans.value(&path);
    let env = f.env;
    let (message, help) = match f.value.trim().strip_prefix(FILE_PREFIX) {
        Some(file) => {
            match std::fs::read_to_string(base_dir.join(file.trim())).map(zeroize::Zeroizing::new) {
                Err(e) => (
                    format!("{name}: cannot read {}: {}", file.trim(), file_reason(&e)),
                    "give a file burrow's user can read, relative to the config file".to_string(),
                ),
                Ok(text) if text.trim().is_empty() => (
                    format!("{name}: {} is empty", file.trim()),
                    "write the value into the file".to_string(),
                ),
                Ok(_) => return None,
            }
        }
        None if !f.inline_ok && !spans.from_env(&path) => (
            format!(
                "{name} is written in the file; use ${{{env}}} or file: so it stays out of the config"
            ),
            format!(
                "for example ${{{env}}}, set in the service's environment, or file:/etc/burrow/{}",
                env.to_ascii_lowercase()
            ),
        ),
        None if f.value.trim().is_empty() => (
            format!("{name} is empty"),
            format!("set it, for example from ${{{env}}}"),
        ),
        None => return None,
    };
    Some(Diagnostic::error(f.code, message).at(at).help(help))
}
