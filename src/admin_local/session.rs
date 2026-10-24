use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use tracing::debug;

use super::token::Token;

pub const KEEP_SECS: u64 = 12 * 3600;
pub const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct Session {
    pub token: Token,
    pub prefix: String,
    pub port: Option<u16>,
    pub reused: bool,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    token: String,
    prefix: String,
    port: u16,
    used: u64,
}

pub fn dir() -> Option<PathBuf> {
    let dir = match std::env::var_os("BURROW_ADMIN_SESSION_DIR") {
        Some(d) => PathBuf::from(d),
        None => dirs::state_dir()
            .or_else(dirs::data_local_dir)?
            .join("burrow")
            .join("admin-sessions"),
    };
    #[cfg(unix)]
    {
        let sudo_uid = std::env::var("SUDO_UID").ok().and_then(|u| u.parse().ok());
        for_sudo(
            dir,
            rustix::process::geteuid().as_raw(),
            sudo_uid,
            root_home,
        )
    }
    #[cfg(not(unix))]
    {
        Some(dir)
    }
}

#[cfg(unix)]
fn root_home() -> Option<PathBuf> {
    nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(0))
        .ok()
        .flatten()
        .map(|u| u.dir)
        .filter(|d| d.is_absolute())
}

#[cfg(unix)]
fn in_home(home: &Path) -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        home.join(".local").join("state")
    };
    base.join("burrow").join("admin-sessions")
}

#[cfg(unix)]
fn for_sudo(
    dir: PathBuf,
    euid: u32,
    sudo_uid: Option<u32>,
    root_home: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let Some(user) = sudo_uid.filter(|u| euid == 0 && *u != 0) else {
        return Some(dir);
    };
    let theirs = |d: &Path| {
        d.ancestors()
            .any(|a| std::fs::metadata(a).is_ok_and(|m| m.uid() == user))
    };
    if !theirs(&dir) {
        return Some(dir);
    }
    root_home().map(|h| in_home(&h)).filter(|d| !theirs(d))
}

fn file_for(dir: &Path, config: &Path) -> PathBuf {
    let key = std::fs::canonicalize(config).unwrap_or_else(|_| config.to_path_buf());
    let digest = ring::digest::digest(&ring::digest::SHA256, key.to_string_lossy().as_bytes());
    dir.join(format!(
        "session-{}.json",
        hex::encode(&digest.as_ref()[..12])
    ))
}

fn valid(s: &Stored) -> bool {
    let hex_of = |t: &str, n: usize| t.len() == n && t.bytes().all(|b| b.is_ascii_hexdigit());
    hex_of(&s.token, 64)
        && s.prefix.strip_prefix("/r-").is_some_and(|h| hex_of(h, 32))
        && s.port != 0
}

impl Session {
    pub fn fresh() -> Session {
        Session {
            token: Token::generate(),
            prefix: super::http::run_prefix(),
            port: None,
            reused: false,
        }
    }
}

pub struct Keep {
    pub dir: PathBuf,
    pub config: PathBuf,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn live(text: &str, now: u64) -> Option<Stored> {
    serde_json::from_str::<Stored>(text)
        .ok()
        .filter(|s| valid(s) && now.saturating_sub(s.used) < KEEP_SECS && s.used <= now + 60)
}

pub fn sweep(dir: &Path, now: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("session-") && name.ends_with(".json")) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_file() {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                debug!("cannot read the admin session {}: {e}", path.display());
                continue;
            }
        };
        if live(&text, now).is_some() {
            continue;
        }
        if let Err(e) = std::fs::remove_file(&path) {
            debug!(
                "cannot delete the old admin session {}: {e}",
                path.display()
            );
        }
    }
}

pub fn load(dir: &Path, config: &Path, now: u64) -> Session {
    let fresh = Session::fresh;
    let Ok(text) = std::fs::read_to_string(file_for(dir, config)) else {
        return fresh();
    };
    match live(&text, now) {
        Some(s) => Session {
            token: Token::from_stored(s.token),
            prefix: s.prefix,
            port: Some(s.port),
            reused: true,
        },
        _ => fresh(),
    }
}

pub fn save(dir: &Path, config: &Path, s: &Session, port: u16, now: u64) -> std::io::Result<()> {
    make_private_dir(dir)?;
    let text = serde_json::to_string(&Stored {
        token: s.token.as_str().to_string(),
        prefix: s.prefix.clone(),
        port,
        used: now,
    })
    .map_err(std::io::Error::other)?;
    crate::instance::write_private(&file_for(dir, config), &text)
}

fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let meta = std::fs::symlink_metadata(dir)?;
        if meta.is_dir()
            && meta.uid() == rustix::process::geteuid().as_raw()
            && meta.mode() & 0o077 != 0
        {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "server: x\n").unwrap();
        p
    }

    #[test]
    fn a_session_is_reused_for_the_same_config_until_it_has_been_unused_for_12_hours() {
        let home = tempfile::tempdir().unwrap();
        let store = home.path().join("state").join("admin");
        let ops = config(home.path(), "ops.yaml");
        let first = load(&store, &ops, 1000);
        assert!(!first.reused);
        save(&store, &ops, &first, 37839, 1000).unwrap();
        let again = load(&store, &ops, 1000 + KEEP_SECS - 1);
        assert!(again.reused);
        assert_eq!(again.token.as_str(), first.token.as_str());
        assert_eq!(again.prefix, first.prefix);
        assert_eq!(again.port, Some(37839));
        let late = load(&store, &ops, 1000 + KEEP_SECS);
        assert!(!late.reused, "expired after 12 h without use");
        assert_ne!(late.token.as_str(), first.token.as_str());
        let other = config(home.path(), "other.yaml");
        assert!(!load(&store, &other, 1000).reused, "one session per config");
    }

    #[cfg(unix)]
    #[test]
    fn under_sudo_the_session_is_kept_in_roots_home_and_never_in_the_users() {
        let me = rustix::process::getuid().as_raw();
        if me == 0 {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let store = home.path().join(".local/state/burrow/admin-sessions");
        let root = PathBuf::from("/burrow-absent-root-home");
        let roots = || Some(PathBuf::from("/burrow-absent-root-home"));
        let in_root = for_sudo(store.clone(), 0, Some(me), roots).unwrap();
        assert!(in_root.starts_with(&root), "{}", in_root.display());
        assert!(
            in_root.ends_with("burrow/admin-sessions"),
            "{}",
            in_root.display()
        );
        assert_eq!(for_sudo(store.clone(), 0, Some(me), || None), None);
        let theirs = || Some(home.path().to_path_buf());
        assert_eq!(for_sudo(store.clone(), 0, Some(me), theirs), None);
        assert_eq!(
            for_sudo(store.clone(), 0, Some(me + 1), roots),
            Some(store.clone())
        );
        assert_eq!(for_sudo(store.clone(), 0, None, roots), Some(store.clone()));
        assert_eq!(
            for_sudo(store.clone(), me, Some(me), roots),
            Some(store.clone())
        );
        assert!(root_home().is_some_and(|h| h.is_absolute()));
        let kept = root.join("admin-sessions");
        assert_eq!(for_sudo(kept.clone(), 0, Some(me), || None), Some(kept));
    }

    #[test]
    fn the_session_file_is_private_and_a_broken_one_is_ignored() {
        let home = tempfile::tempdir().unwrap();
        let store = home.path().join("admin");
        std::fs::create_dir(&store).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let ops = config(home.path(), "ops.yaml");
        let s = load(&store, &ops, 5);
        save(&store, &ops, &s, 4000, 5).unwrap();
        let file = file_for(&store, &ops);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let d = std::fs::metadata(&store).unwrap().permissions().mode();
            assert_eq!(d & 0o777, 0o700);
        }
        for broken in [
            "not json",
            "{\"token\":\"abc\",\"prefix\":\"/r-00\",\"port\":4000,\"used\":5}",
            &format!(
                "{{\"token\":\"{}\",\"prefix\":\"/x\",\"port\":4000,\"used\":5}}",
                "a".repeat(64)
            ),
        ] {
            std::fs::write(&file, broken).unwrap();
            assert!(!load(&store, &ops, 5).reused, "{broken}");
        }
    }

    #[test]
    fn the_sweep_deletes_only_expired_or_broken_session_files() {
        let home = tempfile::tempdir().unwrap();
        let store = home.path().join("admin");
        let now = 100_000;
        let ops = config(home.path(), "ops.yaml");
        let other = config(home.path(), "other.yaml");
        save(&store, &ops, &Session::fresh(), 4000, now - KEEP_SECS - 1).unwrap();
        save(&store, &other, &Session::fresh(), 4001, now - 10).unwrap();
        let expired = file_for(&store, &ops);
        let fresh = file_for(&store, &other);
        let future = store.join("session-future.json");
        let text = std::fs::read_to_string(&fresh).unwrap();
        std::fs::write(
            &future,
            text.replace(
                &format!("\"used\":{}", now - 10),
                &format!("\"used\":{}", now + 61),
            ),
        )
        .unwrap();
        let broken = store.join("session-broken.json");
        std::fs::write(&broken, "not json").unwrap();
        let empty = store.join("session-empty.json");
        std::fs::write(&empty, "").unwrap();
        let unrelated = store.join("notes.json");
        std::fs::write(&unrelated, "not json").unwrap();
        let folder = store.join("session-folder.json");
        std::fs::create_dir(&folder).unwrap();
        #[cfg(unix)]
        let link = {
            let outside = home.path().join("outside.json");
            std::fs::write(&outside, "not json").unwrap();
            let link = store.join("session-link.json");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            (link, outside)
        };
        sweep(&store, now);
        for gone in [&expired, &future, &broken, &empty] {
            assert!(!gone.exists(), "{} should be deleted", gone.display());
        }
        for kept in [&fresh, &unrelated, &folder] {
            assert!(kept.exists(), "{} should stay", kept.display());
        }
        #[cfg(unix)]
        {
            assert!(
                std::fs::symlink_metadata(&link.0).is_ok(),
                "a symlink is never touched"
            );
            assert!(link.1.exists(), "nothing outside the folder is touched");
        }
        assert!(load(&store, &other, now).reused);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_the_sweep_cannot_delete_does_not_stop_the_start() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let store = home.path().join("admin");
        std::fs::create_dir(&store).unwrap();
        std::fs::write(store.join("session-broken.json"), "not json").unwrap();
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o500)).unwrap();
        sweep(&store, 5);
        let ops = config(home.path(), "ops.yaml");
        assert!(!load(&store, &ops, 5).reused);
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(store.join("session-broken.json").exists());
    }
}
