use std::path::{Path, PathBuf};

pub const CRATE: &str = "config-editor-leptos";
const DEFINED_IN: &str = "src/editor.rs";
const CONSTANT: &str = "pub const PRESET_CSS: &str = concat!(";

const INCLUDE: &str = "include_str!(\"";

pub fn parts(source: &str, origin: &str) -> Result<Vec<String>, String> {
    let at = source.find(CONSTANT).ok_or_else(|| {
        format!("{CRATE} ({origin}): {DEFINED_IN} no longer defines PRESET_CSS as a concat!")
    })?;
    let rest = &source[at + CONSTANT.len()..];
    let body = &rest[..rest
        .find(");")
        .ok_or_else(|| format!("{CRATE} ({origin}): PRESET_CSS has no end"))?];
    let mut names = Vec::new();
    let mut left = body;
    loop {
        left = left.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if left.is_empty() {
            break;
        }
        let call = left.strip_prefix(INCLUDE).and_then(|r| r.split_once("\")"));
        let Some((name, after)) = call.filter(|(n, _)| !n.contains('"')) else {
            let shown: String = left.chars().take(60).collect();
            return Err(format!(
                "{CRATE} ({origin}): PRESET_CSS holds more than include_str! calls, so editor.css would miss it: {shown}"
            ));
        };
        names.push(name.to_string());
        left = after;
    }
    if names.is_empty() {
        return Err(format!(
            "{CRATE} ({origin}): PRESET_CSS includes no stylesheet"
        ));
    }
    Ok(names)
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
    let read =
        |p: PathBuf| std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()));
    let source = read(dir.join(DEFINED_IN))?;
    let src = dir.join("src");
    let mut css = String::new();
    for name in parts(&source, &origin)? {
        css.push_str(&read(src.join(name))?);
    }
    Ok(css)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REV: &str = "git+https://github.com/paradoxe35/config-editor?rev=70991fa#70991fa";

    fn constant(body: &str) -> String {
        format!("/// x\n{CONSTANT}{body});\nconst OTHER: &str = include_str!(\"other.css\");\n")
    }

    #[test]
    fn the_stylesheets_come_in_the_order_of_the_constant() {
        let source = constant(
            "\n    include_str!(\"preset.css\"),\n    include_str!(\"themes.css\"),\n    include_str!(\"documents.css\")\n",
        );
        assert_eq!(
            parts(&source, REV).unwrap(),
            ["preset.css", "themes.css", "documents.css"]
        );
        assert_eq!(
            parts(&constant("include_str!(\"a.css\"),"), REV).unwrap(),
            ["a.css"]
        );
    }

    #[test]
    fn anything_but_include_str_calls_is_refused_with_the_crate_source() {
        for body in [
            "include_str!(\"a.css\"), \"b { color: red }\"",
            "\"inline\"",
            "include_str!(\"a.css\"), env!(\"X\")",
            "include_str!(concat!(\"a\", \".css\"))",
            "include_str!(\"a.css\") // note",
        ] {
            let e = parts(&constant(body), REV).expect_err(body);
            assert!(
                e.starts_with(&format!(
                    "{CRATE} ({REV}): PRESET_CSS holds more than include_str! calls"
                )),
                "{e}"
            );
        }
        let e = parts(&constant("include_str!(\"a.css\"), \"x\""), REV).unwrap_err();
        assert!(e.ends_with(": \"x\""), "the unexpected text is shown: {e}");
        assert!(
            parts(&constant(""), REV)
                .unwrap_err()
                .ends_with("includes no stylesheet")
        );
        assert!(
            parts("pub const PRESET_CSS: &str = \"\";", REV)
                .unwrap_err()
                .contains("no longer defines PRESET_CSS")
        );
    }
}
