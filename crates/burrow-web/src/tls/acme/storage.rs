use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

pub const KEEP_REMOVED_SECS: u64 = 30 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub directory: String,
    pub names: Vec<String>,
    pub issued: u64,
}

#[derive(Debug, Clone)]
pub struct Stored {
    pub chain_pem: String,
    pub key_pem: String,
    pub meta: Meta,
}

pub fn account_dir(state: &Path, directory: &str) -> PathBuf {
    state
        .join("acme")
        .join(crate::tls::x509::short_hex(directory.as_bytes(), 8))
}

pub fn name_set(names: &[String]) -> Vec<String> {
    let mut set: Vec<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
    set.sort();
    set.dedup();
    set
}

pub fn cert_dir(state: &Path, names: &[String]) -> PathBuf {
    let set = name_set(names);
    let hex = crate::tls::x509::short_hex(set.join("\n").as_bytes(), 6);
    let first: String = set
        .first()
        .map(String::as_str)
        .unwrap_or("none")
        .chars()
        .take(64)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    state.join("certs").join(format!("{first}-{hex}"))
}

pub fn all(state: &Path) -> Vec<(PathBuf, Stored)> {
    let Ok(entries) = std::fs::read_dir(state.join("certs")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
        .filter_map(|p| load(&p).map(|s| (p, s)))
        .collect()
}

pub fn make_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

pub type Writer = Arc<dyn Fn(&Path, &[u8]) -> std::io::Result<()> + Send + Sync>;

static WRITER: OnceLock<Writer> = OnceLock::new();
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn use_private_writer(writer: Writer) {
    let _ = WRITER.set(writer);
}

pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    make_dir(path.parent().unwrap_or(Path::new(".")))?;
    match WRITER.get() {
        Some(w) => w(path, bytes),
        None => write_atomic(path, bytes),
    }
}

fn temp_for(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}-{n}.tmp", std::process::id()))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_for(path);
    let written = (|| {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        std::fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

pub fn save(dir: &Path, stored: &Stored) -> std::io::Result<()> {
    write_private(&dir.join("key.pem"), stored.key_pem.as_bytes())?;
    write_private(&dir.join("cert.pem"), stored.chain_pem.as_bytes())?;
    let meta = serde_json::to_vec_pretty(&stored.meta).map_err(std::io::Error::other)?;
    write_private(&dir.join("meta.json"), &meta)?;
    let _ = std::fs::remove_file(dir.join("removed"));
    Ok(())
}

pub fn load(dir: &Path) -> Option<Stored> {
    let meta = serde_json::from_slice(&std::fs::read(dir.join("meta.json")).ok()?).ok()?;
    Some(Stored {
        chain_pem: std::fs::read_to_string(dir.join("cert.pem")).ok()?,
        key_pem: std::fs::read_to_string(dir.join("key.pem")).ok()?,
        meta,
    })
}

pub fn sweep(state: &Path, kept: &[PathBuf], now: u64) {
    let Ok(entries) = std::fs::read_dir(state.join("certs")) else {
        return;
    };
    for e in entries.flatten() {
        let dir = e.path();
        let marker = dir.join("removed");
        if kept.contains(&dir) {
            let _ = std::fs::remove_file(&marker);
            continue;
        }
        let since = std::fs::read_to_string(&marker)
            .ok()
            .and_then(|t| t.trim().parse::<u64>().ok());
        match since {
            Some(t) if now.saturating_sub(t) >= KEEP_REMOVED_SECS => {
                let _ = std::fs::remove_dir_all(&dir);
            }
            Some(_) => {}
            None => {
                let _ = write_private(&marker, now.to_string().as_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(names: &[&str]) -> Stored {
        Stored {
            chain_pem: "chain".into(),
            key_pem: "key".into(),
            meta: Meta {
                directory: "https://acme.test/dir".into(),
                names: names.iter().map(|n| n.to_string()).collect(),
                issued: 1,
            },
        }
    }

    #[test]
    fn a_certificate_is_saved_privately_and_read_back() {
        let state = tempfile::tempdir().unwrap();
        let dir = cert_dir(state.path(), &["blog.example.com".into()]);
        save(&dir, &stored(&["blog.example.com"])).unwrap();
        let back = load(&dir).unwrap();
        assert_eq!(back.meta, stored(&["blog.example.com"]).meta);
        assert_eq!(
            (back.chain_pem.as_str(), back.key_pem.as_str()),
            ("chain", "key")
        );
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 3, "no drafts are left: {names:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir.join("key.pem")), 0o600);
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(&state.path().join("certs")), 0o700);
        }
    }

    #[test]
    fn files_that_share_a_stem_never_share_a_draft() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("account.key");
        let json = dir.path().join("account.json");
        assert_ne!(temp_for(&key), temp_for(&json));
        assert_ne!(temp_for(&key), temp_for(&key));
        write_atomic(&key, b"k").unwrap();
        write_atomic(&json, b"j").unwrap();
        assert_eq!(std::fs::read(&key).unwrap(), b"k");
        assert_eq!(std::fs::read(&json).unwrap(), b"j");
    }

    #[test]
    fn a_certificate_folder_is_named_by_its_names_in_any_order() {
        let s = Path::new("/state");
        let names = |v: &[&str]| v.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let ab = cert_dir(s, &names(&["b.example.com", "A.example.com"]));
        assert_eq!(
            ab,
            cert_dir(
                s,
                &names(&["a.example.com", "b.example.com", "a.example.com"])
            )
        );
        assert_ne!(ab, cert_dir(s, &names(&["a.example.com"])));
        assert!(
            ab.to_string_lossy()
                .starts_with("/state/certs/a.example.com-"),
            "{ab:?}"
        );
        let odd = cert_dir(s, &names(&["*.example.com", "../x"]));
        assert_eq!(odd.parent(), Some(Path::new("/state/certs")));
        assert!(!odd.file_name().unwrap().to_str().unwrap().contains('/'));
        let a = account_dir(s, "https://acme-v02.api.letsencrypt.org/directory");
        let b = account_dir(s, "https://acme-staging-v02.api.letsencrypt.org/directory");
        assert_ne!(a, b);
        assert!(a.starts_with("/state/acme"));
    }

    #[test]
    fn an_unused_certificate_is_kept_for_30_days() {
        let state = tempfile::tempdir().unwrap();
        let dir = |n: &str| cert_dir(state.path(), &[n.to_string()]);
        let (blog, gone) = (dir("blog.example.com"), dir("gone.example.com"));
        save(&blog, &stored(&["blog.example.com"])).unwrap();
        save(&gone, &stored(&["gone.example.com"])).unwrap();
        let day = 24 * 3600;
        sweep(state.path(), std::slice::from_ref(&blog), 100 * day);
        assert!(gone.join("removed").exists());
        sweep(state.path(), std::slice::from_ref(&blog), 129 * day);
        assert!(gone.exists());
        sweep(state.path(), &[blog.clone(), gone.clone()], 129 * day);
        assert!(!gone.join("removed").exists(), "back again within 30 days");
        sweep(state.path(), std::slice::from_ref(&blog), 130 * day);
        sweep(state.path(), std::slice::from_ref(&blog), 160 * day);
        assert!(!gone.exists());
        assert!(load(&blog).is_some());
        assert_eq!(all(state.path()).len(), 1);
    }
}
