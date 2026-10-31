use std::path::{Path, PathBuf};

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

pub fn file_for(dir: &Path, config: &Path, prefix: &str, suffix: &str) -> PathBuf {
    let key = std::fs::canonicalize(config).unwrap_or_else(|_| config.to_path_buf());
    let digest = ring::digest::digest(&ring::digest::SHA256, key.to_string_lossy().as_bytes());
    dir.join(format!(
        "{prefix}{}{suffix}",
        hex::encode(&digest.as_ref()[..12])
    ))
}

pub fn make_private_dir(dir: &Path) -> std::io::Result<()> {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

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
}
