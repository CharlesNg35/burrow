use std::path::{Path, PathBuf};

use base64::Engine;
use burrow_admin_proto::plan::Applied;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::admin_link::store;

pub const FOLDER: &str = "mcp-secrets";
const PASSWORD_BYTES: usize = 18;

pub fn folder() -> Result<PathBuf, String> {
    let dir = store::dir()
        .ok_or("cannot find a private folder on this computer for new secrets")?
        .join(FOLDER);
    store::make_private_dir(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

fn safe(word: &str) -> String {
    word.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn tag() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).expect("operating system random source is available");
    hex::encode(bytes)
}

fn keep(dir: &Path, name: &str, text: &str) -> Result<PathBuf, String> {
    let path = dir.join(name);
    crate::base::fs::write_private(&path, text)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(crate::base::fs::canonical_or_same(&path))
}

pub fn password() -> Zeroizing<String> {
    let mut bytes = Zeroizing::new([0u8; PASSWORD_BYTES]);
    getrandom::fill(&mut *bytes).expect("operating system random source is available");
    Zeroizing::new(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(*bytes))
}

pub fn keep_password(dir: &Path, user: &str, password: &str) -> Result<PathBuf, String> {
    let name = format!("password-{}-{}.txt", safe(user), tag());
    keep(dir, &name, &format!("{password}\n"))
}

fn kept(dir: &Result<PathBuf, String>, what: String, fp: &str, name: &str, text: &str) -> Value {
    let file = dir
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|d| keep(d, name, text));
    match file {
        Ok(f) => json!({ "for": what, "fp": fp, "file": f }),
        Err(e) => json!({ "for": what, "fp": fp, "error": e }),
    }
}

pub fn handed_over(applied: Value, dir: &Result<PathBuf, String>) -> Value {
    let Ok(mut a) = serde_json::from_value::<Applied>(applied) else {
        return json!({ "applied": true });
    };
    let mut files = Vec::new();
    for s in &mut a.snippets {
        let config = Zeroizing::new(std::mem::take(&mut s.config));
        let Some(fp) = s.secret_fp.clone().filter(|_| s.with_secret) else {
            continue;
        };
        let name = format!("{}-{}-{}.yaml", safe(&s.kind), safe(&s.name), safe(&fp));
        let what = format!("the connect config of {} {}", s.kind, s.name);
        files.push(kept(dir, what, &fp, &name, &config));
    }
    for s in &mut a.secrets {
        let value = Zeroizing::new(std::mem::take(&mut s.value));
        if a.snippets
            .iter()
            .any(|n| n.secret_fp.as_deref() == Some(s.fp.as_str()))
        {
            continue;
        }
        let name = format!("secret-{}-{}.txt", safe(&s.path.join(".")), safe(&s.fp));
        let what = format!("the secret of {}", s.path.join("."));
        files.push(kept(
            dir,
            what,
            &s.fp,
            &name,
            &format!("{}\n", value.as_str()),
        ));
    }
    let mut v = serde_json::to_value(a).unwrap_or_default();
    if !files.is_empty() {
        v["files"] = files.into();
        v["files_note"] = "these files on this computer hold the new secrets and are readable only by the user; tell the user their paths, never read them".into();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_is_long_random_and_typeable() {
        let (a, b) = (password(), password());
        assert_eq!(a.len(), 24);
        assert_ne!(a.as_str(), b.as_str());
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn an_apply_result_names_private_files_and_never_carries_a_secret() {
        let dir = tempfile::tempdir().unwrap();
        let v = json!({
            "rev": "b2s:2",
            "secrets": [
                {"handle": "h", "path": ["clients", "x/../y", "secret"], "fp": "ab12", "value": "s3cr3t", "shown_once": true},
                {"handle": "k", "path": ["visitors", "bob", "secret"], "fp": "cd34", "value": "t0ps3cr3t", "shown_once": true}
            ],
            "snippets": [{"kind": "client", "name": "x/../y", "config": "secret: s3cr3t", "with_secret": true, "secret_fp": "ab12", "env_var": "Y_SECRET"}]
        });
        let out = handed_over(v, &Ok(dir.path().to_path_buf()));
        let text = out.to_string();
        assert!(
            !text.contains("s3cr3t") && !text.contains("t0ps3cr3t"),
            "{text}"
        );
        assert!(text.contains("ab12") && text.contains("Y_SECRET"), "{text}");
        let files = out["files"].as_array().unwrap();
        assert_eq!(files.len(), 2, "{out}");
        let first = PathBuf::from(files[0]["file"].as_str().unwrap());
        assert_eq!(
            first.parent().unwrap(),
            crate::base::fs::canonical_or_same(dir.path())
        );
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "secret: s3cr3t");
        let second = PathBuf::from(files[1]["file"].as_str().unwrap());
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "t0ps3cr3t\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let away = handed_over(
            json!({ "rev": "r", "secrets": [{"handle": "h", "path": ["a"], "fp": "ab12", "value": "s3cr3t"}] }),
            &Err("no folder".into()),
        );
        assert!(!away.to_string().contains("s3cr3t"), "{away}");
        assert_eq!(away["files"][0]["error"], "no folder");
    }
}
