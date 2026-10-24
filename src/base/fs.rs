use std::io::Write;
use std::path::{Path, PathBuf};

pub fn canonical(p: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(p)
}

pub fn canonical_or_same(p: &Path) -> PathBuf {
    canonical(p).unwrap_or_else(|_| p.to_path_buf())
}

pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        opts.open(path)?.write_all(text.as_bytes())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let draft = path.with_extension("new");
        let _ = std::fs::remove_file(&draft);
        let mut f = opts.share_mode(0).open(&draft)?;
        restrict_on_windows(&draft);
        let written = f.write_all(text.as_bytes());
        drop(f);
        written
            .and_then(|()| std::fs::rename(&draft, path))
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&draft);
            })
    }
    #[cfg(not(any(unix, windows)))]
    opts.open(path)?.write_all(text.as_bytes())
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
        assert!(!path.with_extension("new").exists());
    }
}
