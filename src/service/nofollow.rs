use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::path::Path;

use rustix::fs::{AtFlags, Dir, FileType, Gid, Mode, OFlags, Uid};

use crate::base::failure::Failure;

const FOLDER: OFlags = crate::base::fs::NO_FOLLOW_FOLDER;

pub(crate) fn failed(what: &str, path: &Path, e: impl std::fmt::Display) -> Failure {
    Failure::new(format!("cannot {what} {}: {e}", path.display()))
}

pub fn folder(path: &Path) -> Result<OwnedFd, Failure> {
    rustix::fs::open(path, FOLDER, Mode::empty()).map_err(|e| {
        failed(
            "open",
            path,
            format!("{e}; it must be a folder, not a link"),
        )
    })
}

pub fn made_subfolder(parent: &Path, name: &OsStr) -> Result<OwnedFd, Failure> {
    let path = parent.join(name);
    let holder = folder(parent)?;
    crate::base::fs::folder_at(&holder, name, 0o700)
        .map(|(dir, _)| dir)
        .map_err(|e| {
            failed(
                "make or open",
                &path,
                format!("{e}; it must be a folder, not a link"),
            )
        })
}

pub fn subfolder(
    parent: &Path,
    name: &str,
    mode: u32,
    owner: Option<(u32, u32)>,
) -> Result<(), Failure> {
    let path = parent.join(name);
    let dir = made_subfolder(parent, OsStr::new(name))?;
    let (uid, gid) = owner.unwrap_or((0, 0));
    rustix::fs::fchown(&dir, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)))
        .map_err(|e| failed("give", &path, e))?;
    rustix::fs::fchmod(&dir, Mode::from_bits_truncate(mode as _))
        .map_err(|e| failed("set permissions on", &path, e))
}

fn remove_at(dir: &OwnedFd, name: &OsStr) {
    let Ok(st) = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) else {
        return;
    };
    if FileType::from_raw_mode(st.st_mode) == FileType::Directory {
        if let Ok(sub) = rustix::fs::openat(dir, name, FOLDER, Mode::empty()) {
            empty(&sub);
        }
        let _ = rustix::fs::unlinkat(dir, name, AtFlags::REMOVEDIR);
    } else {
        let _ = rustix::fs::unlinkat(dir, name, AtFlags::empty());
    }
}

fn remove_matching(dir: &OwnedFd, wanted: impl Fn(&OsStr) -> bool) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(entries) = Dir::read_from(dir) else {
        return;
    };
    let names: Vec<std::ffi::CString> = entries
        .flatten()
        .map(|e| e.file_name().to_owned())
        .filter(|n| !matches!(n.as_bytes(), b"." | b".."))
        .collect();
    for n in names {
        let name = OsStr::from_bytes(n.as_bytes());
        if wanted(name) {
            remove_at(dir, name);
        }
    }
}

fn empty(dir: &OwnedFd) {
    remove_matching(dir, |_| true);
}

pub fn plain_files(
    folder: &Path,
    mut each: impl FnMut(&std::ffi::CStr, &OwnedFd, &rustix::fs::Stat),
) -> Option<OwnedFd> {
    let dir = rustix::fs::open(folder, FOLDER, Mode::empty()).ok()?;
    let file = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    for entry in Dir::read_from(&dir).into_iter().flatten().flatten() {
        let Ok(f) = rustix::fs::openat(&dir, entry.file_name(), file, Mode::empty()) else {
            continue;
        };
        if let Ok(st) = rustix::fs::fstat(&f)
            && FileType::from_raw_mode(st.st_mode) == FileType::RegularFile
            && st.st_nlink == 1
        {
            each(entry.file_name(), &f, &st);
        }
    }
    Some(dir)
}

pub fn remove_entry(parent: &Path, name: impl AsRef<OsStr>) {
    if let Ok(dir) = folder(parent) {
        remove_at(&dir, name.as_ref());
    }
}

#[cfg(target_os = "linux")]
pub fn remove_entries(parent: &Path, wanted: impl Fn(&OsStr) -> bool) {
    if let Ok(dir) = folder(parent) {
        remove_matching(&dir, wanted);
    }
}

#[cfg(any(test, target_os = "macos"))]
pub fn empty_folder(path: &Path) {
    if let Ok(dir) = folder(path) {
        empty(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_removed_without_following_links_or_reading_them() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"), "x").unwrap();
        let queue = root.path().join("queue");
        std::fs::create_dir(&queue).unwrap();
        std::os::unix::fs::symlink(&outside, queue.join("link")).unwrap();
        std::fs::create_dir_all(queue.join("deep/er")).unwrap();
        std::fs::write(queue.join("deep/er/f"), "x").unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(queue.join("fifo"))
            .status()
            .unwrap();
        assert!(made.success());
        std::fs::write(queue.join("update-now"), "x").unwrap();
        remove_entry(&queue, "update-now");
        assert!(!queue.join("update-now").exists());
        empty_folder(&queue);
        assert_eq!(std::fs::read_dir(&queue).unwrap().count(), 0);
        assert!(
            outside.join("keep").exists(),
            "a link is removed, never followed"
        );
    }

    #[test]
    fn a_subfolder_that_is_a_link_is_refused_and_its_target_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("queue")).unwrap();
        let mine = rustix::process::getuid().as_raw();
        let gid = rustix::process::getgid().as_raw();
        let err = subfolder(root.path(), "queue", 0o700, Some((mine, gid))).unwrap_err();
        assert!(err.message.contains("not a link"), "{}", err.message);
        use std::os::unix::fs::PermissionsExt;
        let before = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_ne!(before & 0o777, 0o700, "the link's target keeps its mode");
        subfolder(root.path(), "real", 0o700, Some((mine, gid))).unwrap();
        let made = std::fs::metadata(root.path().join("real")).unwrap();
        assert_eq!(made.permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn a_made_subfolder_is_new_and_private_or_the_one_already_there_and_never_a_link() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        made_subfolder(root.path(), OsStr::new("new")).unwrap();
        let made = std::fs::symlink_metadata(root.path().join("new")).unwrap();
        assert!(made.is_dir());
        assert_eq!(made.permissions().mode() & 0o777, 0o700);
        std::fs::create_dir(root.path().join("there")).unwrap();
        std::fs::write(root.path().join("there/keep"), "x").unwrap();
        made_subfolder(root.path(), OsStr::new("there")).unwrap();
        assert!(
            root.path().join("there/keep").exists(),
            "an existing folder is opened as it is"
        );
        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("link")).unwrap();
        let err = made_subfolder(root.path(), OsStr::new("link")).unwrap_err();
        assert!(err.message.contains("not a link"), "{}", err.message);
        let err = made_subfolder(&root.path().join("link"), OsStr::new("inner")).unwrap_err();
        assert!(err.message.contains("not a link"), "{}", err.message);
        assert!(
            !target.join("inner").exists(),
            "nothing is made through a link"
        );
    }
}
