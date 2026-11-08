use std::path::{Path, PathBuf};

pub const CRATE: &str = "config-editor-leptos";
const SHIPPED: &str = "preset.css";

pub fn checked(css: String, origin: &str) -> Result<String, String> {
    if css.trim().is_empty() {
        return Err(format!("{CRATE} ({origin}): {SHIPPED} is empty"));
    }
    Ok(css)
}

fn crate_dir(root: &Path) -> Result<(PathBuf, String), String> {
    let out = crate::cargo()
        .args([
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            "wasm32-unknown-unknown",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let package = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == CRATE)
        .ok_or_else(|| format!("{CRATE} is not in the locked dependencies"))?;
    let manifest = package["manifest_path"]
        .as_str()
        .ok_or_else(|| format!("{CRATE} has no manifest path"))?;
    let origin = package["source"]
        .as_str()
        .or(package["version"].as_str())
        .unwrap_or("unknown source")
        .to_string();
    let dir = Path::new(manifest)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{manifest} has no folder"))?;
    Ok((dir, origin))
}

pub fn preset(root: &Path) -> Result<String, String> {
    let (dir, origin) = crate_dir(root)?;
    let path = dir.join(SHIPPED);
    let css = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "{CRATE} ({origin}) ships no {SHIPPED} at {}: {e}; the editor's styles need it under the strict CSP",
            path.display()
        )
    })?;
    checked(css, &origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_stylesheet_is_refused_with_the_crate_source() {
        let origin = "git+https://github.com/paradoxe35/config-editor?rev=597ee22#597ee22";
        assert_eq!(
            checked(".config-editor { color: red }".into(), origin),
            Ok(".config-editor { color: red }".to_string())
        );
        assert_eq!(
            checked(" \n".into(), origin),
            Err(format!("{CRATE} ({origin}): preset.css is empty"))
        );
    }

    #[test]
    fn the_locked_editor_ships_its_stylesheet() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the repo");
        let css = preset(root).expect("preset.css of the locked editor");
        assert!(css.contains(".config-editor"), "it styles the editor");
    }
}
