use std::path::{Component, Path, PathBuf};

use burrow_admin_proto::codes::ACCESS_LOG_OUTSIDE;
use burrow_admin_proto::diag::{Diag, Severity};
use burrow_config::{ServerConfig, WebConfig};
use serde_json::Value;

use crate::admin::core::Core;

fn file_of(doc: &Value, path: &[String]) -> Option<String> {
    let v = burrow_admin_proto::value::get(doc, path)?;
    v.as_str()
        .or_else(|| v.get("file").and_then(Value::as_str))
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
}

fn inside(file: &Path, folder: &Path) -> bool {
    let canonical = crate::base::fs::canonical_or_same(folder);
    file.starts_with(folder) && file != folder || file.starts_with(&canonical) && file != canonical
}

pub(super) fn allowed(file: &str, config_dir: &Path, folders: &[PathBuf]) -> bool {
    let file = Path::new(file);
    let full = if file.is_absolute() {
        file.to_path_buf()
    } else {
        config_dir.join(file)
    };
    !full.components().any(|c| c == Component::ParentDir)
        && folders.iter().any(|f| inside(&full, f))
}

pub(super) fn outside(core: &Core, before: &Value, after: &Value) -> Vec<Diag> {
    let path = ServerConfig::WEB.then(WebConfig::ACCESS_LOG).path();
    let Some(file) = file_of(after, &path) else {
        return Vec::new();
    };
    if file_of(before, &path).as_deref() == Some(file.as_str()) {
        return Vec::new();
    }
    let config_dir = core
        .store
        .path()
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let folders: Vec<PathBuf> = std::iter::once(core.store.state_dir())
        .chain(core.store.log_dir())
        .collect();
    if allowed(&file, &config_dir, &folders) {
        return Vec::new();
    }
    let examples: Vec<String> = folders
        .iter()
        .map(|f| f.join("access.log").display().to_string())
        .collect();
    vec![Diag {
        severity: Severity::Error,
        code: ACCESS_LOG_OUTSIDE.into(),
        message: format!(
            "burrow admin writes the access log only into this server's own folders; a file elsewhere, such as a shell's startup file, could run code, so set {file} in server.yaml"
        ),
        path: Some(path),
        also: Vec::new(),
        subjects: Vec::new(),
        label: None,
        help: Some(format!("from here, use {}", examples.join(" or "))),
        line: None,
        col: None,
        end_line: None,
        end_col: None,
        before: false,
        file: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_files_inside_the_servers_own_folders_are_allowed() {
        let folders = [
            PathBuf::from("/var/lib/burrow/home"),
            PathBuf::from("/var/log/burrow/home"),
        ];
        let config = Path::new("/etc/burrow/home");
        let ok = |f: &str| allowed(f, config, &folders);
        assert!(ok("/var/lib/burrow/home/access.log"));
        assert!(ok("/var/log/burrow/home/web/access.log"));
        assert!(!ok("/var/log/burrow/home"));
        assert!(!ok("/var/log/burrow/home/../../../../home/u/.bashrc"));
        assert!(!ok("/home/u/.bashrc"));
        assert!(!ok("/home/u/.profile"));
        assert!(!ok("logs/access.log"));
        assert!(!ok("/var/lib/burrow/homeother/access.log"));
        let beside = Path::new("/var/lib/burrow/home");
        assert!(allowed("logs/access.log", beside, &folders));
    }
}
