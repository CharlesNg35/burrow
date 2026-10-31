use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, Dir, FileType, Gid, Mode, OFlags, Uid};

const FOLDER: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const FILE: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::NOCTTY)
    .union(OFlags::CLOEXEC);

pub fn own_tree(dir: &Path, uid: u32, gid: u32) -> Vec<PathBuf> {
    let mut skipped = Vec::new();
    if let Ok(fd) = rustix::fs::open(dir, FOLDER, Mode::empty()) {
        own_folder(
            &fd,
            dir,
            (Uid::from_raw(uid), Gid::from_raw(gid)),
            &mut skipped,
        );
    }
    skipped
}

fn own_folder(fd: &OwnedFd, at: &Path, ids: (Uid, Gid), skipped: &mut Vec<PathBuf>) {
    let _ = rustix::fs::fchown(fd, Some(ids.0), Some(ids.1));
    let Ok(entries) = Dir::read_from(fd) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let path = at.join(OsStr::from_bytes(name.to_bytes()));
        let Ok(st) = rustix::fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW) else {
            continue;
        };
        match FileType::from_raw_mode(st.st_mode) {
            FileType::Directory => {
                if let Ok(sub) = rustix::fs::openat(fd, name, FOLDER, Mode::empty()) {
                    own_folder(&sub, &path, ids, skipped);
                }
            }
            FileType::RegularFile => own_file(fd, name, ids, path, skipped),
            _ => {}
        }
    }
}

fn own_file(
    dir: &OwnedFd,
    name: &std::ffi::CStr,
    ids: (Uid, Gid),
    path: PathBuf,
    skipped: &mut Vec<PathBuf>,
) {
    let Ok(f) = rustix::fs::openat(dir, name, FILE, Mode::empty()) else {
        return;
    };
    match rustix::fs::fstat(&f) {
        Ok(st) if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile => {}
        Ok(st) if st.st_nlink == 1 => {
            let _ = rustix::fs::fchown(&f, Some(ids.0), Some(ids.1));
        }
        Ok(_) => skipped.push(path),
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> (u32, u32) {
        (
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw(),
        )
    }

    #[test]
    fn the_walk_reaches_nested_files_and_skips_hard_links_and_folder_links() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let dir = root.path().join("state");
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/deep.pem"), b"x").unwrap();
        std::fs::write(dir.join("top.json"), b"x").unwrap();
        std::fs::write(outside.path().join("linked"), b"x").unwrap();
        std::fs::hard_link(outside.path().join("linked"), dir.join("a/b/hard")).unwrap();
        std::fs::write(outside.path().join("beyond"), b"x").unwrap();
        std::fs::hard_link(
            outside.path().join("beyond"),
            outside.path().join("beyond2"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.join("out")).unwrap();
        let (uid, gid) = me();
        let skipped = own_tree(&dir, uid, gid);
        assert_eq!(
            skipped,
            vec![dir.join("a/b/hard")],
            "the nested hard link is skipped and the linked folder is not entered"
        );
    }

    #[test]
    fn a_fifo_in_the_folder_is_skipped_without_waiting() {
        let root = tempfile::tempdir().unwrap();
        let fifo = root.path().join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        let (uid, gid) = me();
        let path = root.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(own_tree(&path, uid, gid)).unwrap());
        let skipped = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the walk does not wait on the fifo");
        assert!(skipped.is_empty());
    }

    #[test]
    fn a_link_given_as_the_folder_itself_is_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("f"), b"x").unwrap();
        std::fs::hard_link(outside.path().join("f"), outside.path().join("g")).unwrap();
        let link = root.path().join("state");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let (uid, gid) = me();
        assert!(own_tree(&link, uid, gid).is_empty());
        assert!(!own_tree(outside.path(), uid, gid).is_empty());
    }
}
