use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::tls::acme::storage::write_private;

pub const GIVE_UP_SECS: u64 = 7 * 24 * 3600;

static FILE: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Left {
    pub name: String,
    pub fqdn: String,
    pub value: String,
    pub since: u64,
}

pub fn path(state: &Path) -> PathBuf {
    state.join("dns-pending.json")
}

pub fn load(path: &Path) -> Vec<Left> {
    let _held = FILE.lock().expect("pending lock");
    read(path)
}

fn read(path: &Path) -> Vec<Left> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write(path: &Path, list: &[Left]) -> std::io::Result<()> {
    if list.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let bytes = serde_json::to_vec_pretty(list).map_err(std::io::Error::other)?;
    write_private(path, &bytes)
}

pub fn add(path: &Path, left: &Left) -> std::io::Result<()> {
    let _held = FILE.lock().expect("pending lock");
    let mut list = read(path);
    if !list
        .iter()
        .any(|l| l.fqdn == left.fqdn && l.value == left.value)
    {
        list.push(left.clone());
    }
    write(path, &list)
}

pub fn remove(path: &Path, fqdn: &str, value: &str) -> std::io::Result<()> {
    let _held = FILE.lock().expect("pending lock");
    let mut list = read(path);
    list.retain(|l| !(l.fqdn == fqdn && l.value == value));
    write(path, &list)
}

pub fn stale(list: &[Left], now: u64) -> Vec<Left> {
    list.iter()
        .filter(|l| now >= l.since + GIVE_UP_SECS)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn left(fqdn: &str, value: &str, since: u64) -> Left {
        Left {
            name: "wiki.example.com".into(),
            fqdn: fqdn.into(),
            value: value.into(),
            since,
        }
    }

    #[test]
    fn values_are_listed_until_their_cleanup_and_the_file_goes_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = path(dir.path());
        assert!(load(&p).is_empty());
        let a = left("_acme-challenge.wiki.example.com.", "v1", 10);
        let b = left("_acme-challenge.wiki.example.com.", "v2", 10);
        add(&p, &a).unwrap();
        add(&p, &b).unwrap();
        add(&p, &a).unwrap();
        assert_eq!(load(&p), vec![a.clone(), b.clone()]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        remove(&p, &a.fqdn, "v1").unwrap();
        assert_eq!(load(&p), vec![b.clone()]);
        remove(&p, &b.fqdn, "v2").unwrap();
        assert!(!p.exists());
        remove(&p, &b.fqdn, "v2").unwrap();
        assert_eq!(stale(std::slice::from_ref(&a), 10 + GIVE_UP_SECS), vec![a.clone()]);
        assert!(stale(&[a], 11).is_empty());
    }
}
