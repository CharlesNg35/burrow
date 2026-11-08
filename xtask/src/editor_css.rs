use std::path::{Path, PathBuf};

pub const CRATE: &str = "config-editor-leptos";
const DEFINED_IN: &str = "src/editor.rs";
const CONSTANT: &str = "pub const PRESET_CSS: &str = concat!(";

pub fn parts(source: &str) -> Result<Vec<String>, String> {
    let at = source.find(CONSTANT).ok_or_else(|| {
        format!("{CRATE}'s {DEFINED_IN} no longer defines PRESET_CSS as a concat!")
    })?;
    let rest = &source[at + CONSTANT.len()..];
    let body = &rest[..rest.find(");").ok_or("PRESET_CSS has no end")?];
    let names: Vec<String> = body
        .split("include_str!(\"")
        .skip(1)
        .filter_map(|p| p.split_once("\")").map(|(name, _)| name.to_string()))
        .collect();
    if names.is_empty() {
        return Err("PRESET_CSS includes no stylesheet".into());
    }
    Ok(names)
}

fn crate_dir(root: &Path) -> Result<PathBuf, String> {
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
    let manifest = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == CRATE)
        .and_then(|p| p["manifest_path"].as_str())
        .ok_or_else(|| format!("{CRATE} is not in the locked dependencies"))?;
    Path::new(manifest)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{manifest} has no folder"))
}

pub fn preset(root: &Path) -> Result<String, String> {
    let dir = crate_dir(root)?;
    let read =
        |p: PathBuf| std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()));
    let source = read(dir.join(DEFINED_IN))?;
    let src = dir.join("src");
    let mut css = String::new();
    for name in parts(&source)? {
        css.push_str(&read(src.join(name))?);
    }
    Ok(css)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stylesheets_come_in_the_order_of_the_constant() {
        let source = "/// x\npub const PRESET_CSS: &str = concat!(\n    include_str!(\"preset.css\"),\n    include_str!(\"themes.css\"),\n    include_str!(\"documents.css\")\n);\nconst OTHER: &str = include_str!(\"other.css\");\n";
        assert_eq!(
            parts(source).unwrap(),
            ["preset.css", "themes.css", "documents.css"]
        );
        assert!(parts("pub const PRESET_CSS: &str = \"\";").is_err());
        assert!(parts("pub const PRESET_CSS: &str = concat!(\"a\");").is_err());
    }
}
