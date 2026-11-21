use std::path::Path;

pub const EMIT_RELOCS: &str = "-Clink-arg=--emit-relocs";
const SEP: char = '\u{1f}';

pub fn configured(config_toml: &str, target: &str) -> Result<Vec<String>, String> {
    let table: toml::Table = config_toml
        .parse()
        .map_err(|e| format!(".cargo/config.toml: {e}"))?;
    let Some(flags) = table
        .get("target")
        .and_then(|t| t.get(target))
        .and_then(|t| t.get("rustflags"))
    else {
        return Ok(Vec::new());
    };
    match flags {
        toml::Value::String(s) => Ok(s.split_whitespace().map(str::to_string).collect()),
        toml::Value::Array(a) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("target.{target}.rustflags holds a non-string"))
            })
            .collect(),
        _ => Err(format!("target.{target}.rustflags is not a list")),
    }
}

pub fn from_env(encoded: Option<&str>, plain: Option<&str>) -> Option<Vec<String>> {
    if let Some(e) = encoded {
        return Some(
            e.split(SEP)
                .filter(|f| !f.is_empty())
                .map(str::to_string)
                .collect(),
        );
    }
    plain.map(|p| p.split_whitespace().map(str::to_string).collect())
}

fn contains_run(all: &[String], run: &[String]) -> bool {
    run.is_empty() || all.windows(run.len()).any(|w| w == run)
}

pub fn merged(user: Vec<String>, configured: &[String]) -> String {
    let mut all = user;
    let ours = [configured.to_vec(), vec![EMIT_RELOCS.to_string()]];
    for run in ours {
        if !contains_run(&all, &run) {
            all.extend(run);
        }
    }
    all.join(&SEP.to_string())
}

pub fn for_wasm(root: &Path, target: &str) -> Result<Option<String>, String> {
    let encoded = std::env::var("CARGO_ENCODED_RUSTFLAGS").ok();
    let plain = std::env::var("RUSTFLAGS").ok();
    let Some(user) = from_env(encoded.as_deref(), plain.as_deref()) else {
        return Ok(None);
    };
    let path = root.join(".cargo/config.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    Ok(Some(merged(user, &configured(&text, target)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "[alias]\nxtask = \"run\"\n\n[target.wasm32-unknown-unknown]\nrustflags = [\"--cfg\", \"erase_components\"]\n";

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn the_config_flags_of_the_target_are_read() {
        let wasm = configured(CONFIG, "wasm32-unknown-unknown").unwrap();
        assert_eq!(wasm, v(&["--cfg", "erase_components"]));
        assert!(
            configured(CONFIG, "x86_64-unknown-linux-gnu")
                .unwrap()
                .is_empty()
        );
        let one = "[target.wasm32-unknown-unknown]\nrustflags = \"--cfg a -Cfoo\"\n";
        assert_eq!(
            configured(one, "wasm32-unknown-unknown").unwrap(),
            v(&["--cfg", "a", "-Cfoo"])
        );
        assert!(configured("not toml [", "x").is_err());
    }

    #[test]
    fn no_flags_in_the_environment_leave_the_build_to_the_config() {
        assert_eq!(from_env(None, None), None);
        assert_eq!(
            from_env(None, Some(" -Cfoo  --cfg x ")),
            Some(v(&["-Cfoo", "--cfg", "x"]))
        );
        assert_eq!(
            from_env(Some("-Cfoo\u{1f}--cfg\u{1f}a b"), Some("-Cignored")),
            Some(v(&["-Cfoo", "--cfg", "a b"]))
        );
        assert_eq!(
            from_env(Some(""), None),
            Some(Vec::new()),
            "an empty CARGO_ENCODED_RUSTFLAGS is still set"
        );
        assert_eq!(from_env(None, Some("")), Some(Vec::new()));
    }

    #[test]
    fn user_flags_come_first_and_ours_are_added_once() {
        let cfg = v(&["--cfg", "erase_components"]);
        assert_eq!(
            merged(v(&["-Cdebuginfo=1"]), &cfg),
            "-Cdebuginfo=1\u{1f}--cfg\u{1f}erase_components\u{1f}-Clink-arg=--emit-relocs"
        );
        assert_eq!(
            merged(v(&["--cfg", "erase_components"]), &cfg),
            "--cfg\u{1f}erase_components\u{1f}-Clink-arg=--emit-relocs",
            "RUSTFLAGS=\"--cfg erase_components\" is not doubled"
        );
        assert_eq!(merged(Vec::new(), &[]), "-Clink-arg=--emit-relocs");
        assert_eq!(
            merged(v(&["-Clink-arg=--emit-relocs", "--cfg", "x"]), &[]),
            "-Clink-arg=--emit-relocs\u{1f}--cfg\u{1f}x"
        );
    }
}
