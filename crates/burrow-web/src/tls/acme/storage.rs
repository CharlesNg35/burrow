use std::io::Write;
use std::path::{Path, PathBuf};

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
    let digest = ring::digest::digest(&ring::digest::SHA256, directory.as_bytes());
    let hex: String = digest.as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    state.join("acme").join(hex)
}

pub fn group_dir(state: &Path, group: &str) -> PathBuf {
    let clean: String = group
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = if clean.chars().all(|c| c == '.') {
        format!("_{clean}")
    } else {
        clean
    };
    state.join("certs").join(clean)
}

pub fn make_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    make_dir(dir)?;
    let tmp = path.with_extension("tmp");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
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
        let dir = group_dir(state.path(), "blog");
        save(&dir, &stored(&["blog.example.com"])).unwrap();
        let back = load(&dir).unwrap();
        assert_eq!(back.meta, stored(&["blog.example.com"]).meta);
        assert_eq!(
            (back.chain_pem.as_str(), back.key_pem.as_str()),
            ("chain", "key")
        );
        assert!(!dir.join("key.tmp").exists());
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
    fn folder_names_stay_inside_the_state_folder() {
        let s = Path::new("/state");
        assert_eq!(group_dir(s, "blog"), Path::new("/state/certs/blog"));
        assert_eq!(group_dir(s, "../x"), Path::new("/state/certs/.._x"));
        assert_eq!(group_dir(s, ".."), Path::new("/state/certs/_.."));
        assert_eq!(
            group_dir(s, "websocket.host"),
            Path::new("/state/certs/websocket.host")
        );
        let a = account_dir(s, "https://acme-v02.api.letsencrypt.org/directory");
        let b = account_dir(s, "https://acme-staging-v02.api.letsencrypt.org/directory");
        assert_ne!(a, b);
        assert!(a.starts_with("/state/acme"));
    }

    #[test]
    fn a_removed_service_keeps_its_certificate_for_30_days() {
        let state = tempfile::tempdir().unwrap();
        let (blog, gone) = (
            group_dir(state.path(), "blog"),
            group_dir(state.path(), "gone"),
        );
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
    }
}
