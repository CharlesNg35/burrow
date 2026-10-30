use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn canonical(p: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(p)
}

pub fn canonical_or_same(p: &Path) -> PathBuf {
    canonical(p).unwrap_or_else(|_| p.to_path_buf())
}

pub const TEMP_EXTENSION: &str = "tmp";

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn temp_for(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(
        ".{name}.{}-{n}.{TEMP_EXTENSION}",
        std::process::id()
    ))
}

pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    write_atomic(path, text.as_bytes(), 0o600)
}

pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    swap_in(path, &temp_for(path), mode, mode & 0o077 == 0, |f| {
        f.write_all(bytes)
    })
}

pub fn copy_atomic(from: &Path, to: &Path, tmp: &Path, mode: u32) -> std::io::Result<()> {
    let mut source = std::fs::File::open(from)?;
    swap_in(to, tmp, mode, false, |f| {
        std::io::copy(&mut source, f).map(|_| ())
    })
}

fn swap_in(
    path: &Path,
    tmp: &Path,
    mode: u32,
    private: bool,
    fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let written = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, mode);
        #[cfg(windows)]
        std::os::windows::fs::OpenOptionsExt::share_mode(&mut opts, 0);
        let mut f = opts.open(tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(windows)]
        if private {
            restrict_on_windows(tmp);
        }
        #[cfg(not(windows))]
        let _ = private;
        #[cfg(not(unix))]
        let _ = mode;
        fill(&mut f)?;
        f.sync_all()?;
        drop(f);
        replace(tmp, path)?;
        sync_folder(path);
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    written
}

fn sync_folder(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    #[cfg(not(unix))]
    let _ = path;
}

pub fn replace(tmp: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    let mut moved_aside = None;
    loop {
        match std::fs::rename(tmp, to) {
            Ok(()) => {
                if let Some(aside) = &moved_aside {
                    let _ = std::fs::remove_file(aside);
                }
                return Ok(());
            }
            Err(e)
                if cfg!(windows)
                    && tries < 8
                    && e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(e)
                if cfg!(windows)
                    && tries == 8
                    && e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                tries += 1;
                let aside = to.with_extension("old");
                let _ = std::fs::remove_file(&aside);
                if std::fs::rename(to, &aside).is_ok() {
                    moved_aside = Some(aside);
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(tmp);
                if let Some(aside) = &moved_aside {
                    let _ = std::fs::rename(aside, to);
                }
                return Err(e);
            }
        }
    }
}

#[cfg(windows)]
pub(crate) fn restrict_on_windows(path: &Path) {
    let me = std::process::Command::new("whoami")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|u| !u.is_empty());
    let mut args = vec![
        path.display().to_string(),
        "/inheritance:r".into(),
        "/grant:r".into(),
        "*S-1-5-18:F".into(),
        "*S-1-5-32-544:F".into(),
    ];
    if let Some(me) = me {
        args.push(format!("{me}:F"));
    }
    let _ = std::process::Command::new("icacls").args(&args).output();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_paths_are_plain_paths() {
        let dir = tempfile::tempdir().unwrap();
        let c = canonical(dir.path()).unwrap();
        assert!(c.is_absolute());
        assert!(!c.to_string_lossy().starts_with(r"\\?\"), "{c:?}");
        assert_eq!(c, canonical(&c).unwrap());
    }

    #[test]
    fn private_files_are_replaced_whole_through_their_own_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("account.key");
        let json = dir.path().join("account.json");
        assert_ne!(temp_for(&key), temp_for(&json));
        assert_ne!(temp_for(&key), temp_for(&key));
        write_private(&key, "one").unwrap();
        write_private(&json, "{}").unwrap();
        write_private(&key, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&key).unwrap(), "two");
        assert_eq!(std::fs::read_to_string(&json).unwrap(), "{}");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(TEMP_EXTENSION))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&key).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            write_atomic(&json, b"{}", 0o644).unwrap();
            let mode = std::fs::metadata(&json).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o644);
            let target = dir.path().join("elsewhere");
            std::fs::write(&target, "keep").unwrap();
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            write_private(&link, "new").unwrap();
            assert_eq!(
                std::fs::read_to_string(&target).unwrap(),
                "keep",
                "a link is replaced, never followed"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_private_file_is_locked_down_before_it_holds_anything() {
        let dir = tempfile::tempdir().unwrap();
        let shared = std::process::Command::new("icacls")
            .arg(dir.path())
            .args(["/grant", "*S-1-5-32-545:(OI)(CI)M"])
            .output()
            .unwrap();
        assert!(shared.status.success());
        let path = dir.path().join("1.status");
        write_private(&path, "token").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "token");
        let acl = std::process::Command::new("icacls")
            .arg(&path)
            .output()
            .unwrap();
        let acl = String::from_utf8_lossy(&acl.stdout).to_string();
        assert!(
            !acl.contains("S-1-5-32-545") && !acl.contains("BUILTIN\\Users"),
            "{acl}"
        );
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no draft is left"
        );
    }
}
