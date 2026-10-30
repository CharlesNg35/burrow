use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{default_dir, dirs_with, held_by_a_trusted_instance};

pub(super) const NOTE_PATIENCE: std::time::Duration = std::time::Duration::from_secs(1);

pub(super) fn no_follow(o: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    o
}

#[cfg(unix)]
pub(super) fn extra_links(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.nlink() > 1)
}

#[cfg(not(unix))]
pub(super) fn extra_links(_path: &Path) -> bool {
    false
}

pub(super) fn replace_linked(path: &Path) -> std::io::Result<Option<(File, bool)>> {
    let old = no_follow(OpenOptions::new().read(true)).open(path)?;
    match old.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) if held_by_a_trusted_instance(path) => return Ok(None),
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Error(e)) => return Err(e),
    }
    if crate::base::fs::names(path, &old) {
        std::fs::remove_file(path)?;
    }
    let opened = open_shared(path).map(Some);
    drop(old);
    opened
}

pub(super) fn single_file(f: File) -> std::io::Result<File> {
    let meta = f.metadata()?;
    #[cfg(unix)]
    let linked = std::os::unix::fs::MetadataExt::nlink(&meta) > 1;
    #[cfg(not(unix))]
    let linked = false;
    if !meta.is_file() || linked {
        return Err(std::io::Error::other("not a plain file; remove it"));
    }
    Ok(f)
}

pub(super) fn open_shared(path: &Path) -> std::io::Result<(File, bool)> {
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(f) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                f.set_permissions(std::fs::Permissions::from_mode(0o666))?;
            }
            return Ok((f, true));
        }
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        Err(_) => {}
    }
    match no_follow(OpenOptions::new().read(true).write(true)).open(path) {
        Ok(f) => Ok((single_file(f)?, true)),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok((
            single_file(no_follow(OpenOptions::new().read(true)).open(path)?)?,
            false,
        )),
        Err(e) => Err(e),
    }
}

pub(super) fn write_note(path: &Path, note: &str) -> bool {
    let Ok((mut f, true)) = open_shared(path) else {
        return false;
    };
    let written = f.set_len(0).is_ok() && f.write_all(note.as_bytes()).is_ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o644));
    }
    written
}

pub struct Running {
    pub pid: u32,
    pub config: PathBuf,
}

pub fn running() -> Vec<Running> {
    dirs_with(&default_dir())
        .iter()
        .flat_map(|d| running_in(d))
        .collect()
}

pub fn running_in(dir: &Path) -> Vec<Running> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Running> = listing
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == super::NOTE_EXTENSION))
        .filter_map(|p| trusted_note(&p))
        .collect();
    found.sort_by(|a, b| a.config.cmp(&b.config));
    found
}

fn parse_note(text: &str) -> Option<Running> {
    let (pid, config) = text.trim().strip_prefix("pid ")?.split_once(", config ")?;
    Some(Running {
        pid: pid.parse().ok()?,
        config: PathBuf::from(config),
    })
}

#[cfg(unix)]
pub(super) fn trusted_note(path: &Path) -> Option<Running> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let m = file.metadata().ok()?;
    if !m.is_file() || m.len() > 4096 || m.mode() & 0o022 != 0 {
        return None;
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::Read::take(&mut file, 4097), &mut text).ok()?;
    if text.len() > 4096 {
        return None;
    }
    let note = parse_note(&text)?;
    (trusted_owner(m.uid(), &note.config) && runs_as(note.pid, m.uid())).then_some(note)
}

#[cfg(not(unix))]
pub(super) fn trusted_note(path: &Path) -> Option<Running> {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() <= 4096) {
        return None;
    }
    let mut text = String::new();
    std::io::Read::read_to_string(
        &mut std::io::Read::take(std::fs::File::open(path).ok()?, 4097),
        &mut text,
    )
    .ok()?;
    if text.len() > 4096 {
        return None;
    }
    let note = parse_note(&text)?;
    runs_as(note.pid, 0).then_some(note)
}

#[cfg(unix)]
fn trusted_owner(uid: u32, config: &Path) -> bool {
    let me = rustix::process::geteuid().as_raw();
    uid == 0
        || uid == me
        || crate::install::all_manifests().iter().any(|m| {
            (m.created_account || crate::base::fs::canonical_or_same(&m.config) == config)
                && crate::base::process::account_ids(&m.account).is_ok_and(|(id, _)| id == uid)
        })
}

#[cfg(target_os = "linux")]
fn pid_owner(pid: u32) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(format!("/proc/{pid}"))
        .ok()
        .map(|m| m.uid())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn pid_owner(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-o", "uid=", "-p", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(unix)]
fn runs_as(pid: u32, owner: u32) -> bool {
    pid_owner(pid) == Some(owner)
}

#[cfg(windows)]
fn runs_as(pid: u32, _owner: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| {
            let text = String::from_utf8_lossy(&o.stdout).to_ascii_lowercase();
            text.starts_with("\"burrow") && text.contains(&format!("\"{pid}\""))
        })
}

#[cfg(not(any(unix, windows)))]
fn runs_as(_pid: u32, _owner: u32) -> bool {
    false
}

#[cfg(unix)]
pub(crate) fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    rustix::process::Pid::from_raw(pid).is_some_and(|pid| {
        matches!(
            rustix::process::test_kill_process(pid),
            Ok(()) | Err(rustix::io::Errno::PERM)
        )
    })
}
