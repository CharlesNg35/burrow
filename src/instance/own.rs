use std::path::{Path, PathBuf};

#[cfg(unix)]
fn me() -> u32 {
    rustix::process::geteuid().as_raw()
}

#[cfg(unix)]
pub(super) fn private_to(dir: &Path, me: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(dir)
        .is_ok_and(|m| m.is_dir() && m.uid() == me && m.mode() & 0o077 == 0)
}

#[cfg(unix)]
pub(super) fn own_dir_in(runtime: Option<PathBuf>, temp: &Path, me: u32) -> PathBuf {
    match runtime.filter(|d| d.is_absolute() && private_to(d, me)) {
        Some(run) => run.join("burrow"),
        None => temp.join(format!("burrow-{me}")),
    }
}

#[cfg(unix)]
pub fn own_dir() -> PathBuf {
    own_dir_in(dirs::runtime_dir(), &std::env::temp_dir(), me())
}

#[cfg(not(unix))]
pub fn own_dir() -> PathBuf {
    std::env::temp_dir().join("burrow")
}

#[cfg(unix)]
pub(super) fn prepare_own_as(dir: &Path, me: u32) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    if private_to(dir, me) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "it is a link, or a folder that another user owns or can open",
        ))
    }
}

#[cfg(unix)]
pub fn prepare_own(dir: &Path) -> std::io::Result<()> {
    prepare_own_as(dir, me())
}

#[cfg(not(unix))]
pub fn prepare_own(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}
