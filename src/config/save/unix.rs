use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};

use super::{Refused, spare_prefix};

#[cfg(test)]
thread_local! {
    pub static FAIL_FSYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub static FAIL_LINK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn sync_folder(dir: &OwnedFd) {
    let synced = || -> std::io::Result<()> {
        #[cfg(test)]
        if FAIL_FSYNC.with(|f| f.get()) {
            return Err(std::io::Error::other("injected fsync failure"));
        }
        Ok(rustix::fs::fsync(dir)?)
    };
    if let Err(e) = synced() {
        tracing::warn!(
            "the config folder could not be synced to disk after a save ({e}); the save itself is done"
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Replace,
    Create,
}

pub struct Pending {
    dir: OwnedFd,
    temp: String,
    name: OsString,
    made: Option<std::path::PathBuf>,
    kind: Kind,
}

impl Pending {
    pub fn commit(&self) -> std::io::Result<()> {
        let moved = match self.kind {
            Kind::Replace => rustix::fs::renameat(
                &self.dir,
                self.temp.as_str(),
                &self.dir,
                self.name.as_os_str(),
            ),
            Kind::Create => self.link(),
        };
        if moved.is_err() || self.kind == Kind::Create {
            self.discard();
        }
        moved?;
        sync_folder(&self.dir);
        Ok(())
    }

    fn link(&self) -> rustix::io::Result<()> {
        let linked = {
            #[cfg(test)]
            let failed = FAIL_LINK.with(|f| f.get());
            #[cfg(not(test))]
            let failed = false;
            if failed {
                Err(rustix::io::Errno::PERM)
            } else {
                rustix::fs::linkat(
                    &self.dir,
                    self.temp.as_str(),
                    &self.dir,
                    self.name.as_os_str(),
                    AtFlags::empty(),
                )
            }
        };
        match linked {
            Err(e) if e != rustix::io::Errno::EXIST => self.rename_no_replace(),
            other => other,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    fn rename_no_replace(&self) -> rustix::io::Result<()> {
        rustix::fs::renameat_with(
            &self.dir,
            self.temp.as_str(),
            &self.dir,
            self.name.as_os_str(),
            rustix::fs::RenameFlags::NOREPLACE,
        )
    }

    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    fn rename_no_replace(&self) -> rustix::io::Result<()> {
        if rustix::fs::statat(&self.dir, self.name.as_os_str(), AtFlags::SYMLINK_NOFOLLOW).is_ok() {
            return Err(rustix::io::Errno::EXIST);
        }
        rustix::fs::renameat(
            &self.dir,
            self.temp.as_str(),
            &self.dir,
            self.name.as_os_str(),
        )
    }

    pub fn discard(&self) {
        let _ = rustix::fs::unlinkat(&self.dir, self.temp.as_str(), AtFlags::empty());
    }

    pub fn made(&self) -> Option<&Path> {
        self.made.as_deref()
    }
}

fn split(real: &Path) -> Result<(&Path, &std::ffi::OsStr), Refused> {
    match (real.parent(), real.file_name()) {
        (Some(folder), Some(name)) => Ok((folder, name)),
        _ => Err(Refused::Io(std::io::ErrorKind::InvalidInput.into())),
    }
}

fn temp_name(name: &std::ffi::OsStr) -> String {
    format!(
        "{}{}",
        spare_prefix(&name.to_string_lossy()),
        std::process::id()
    )
}

fn read_at(dir: &OwnedFd, name: &std::ffi::OsStr) -> std::io::Result<String> {
    let mut now = String::new();
    std::fs::File::from(
        rustix::fs::openat(
            dir,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    )
    .read_to_string(&mut now)?;
    Ok(now)
}

pub(super) fn owner_change(
    owner: (u32, u32),
    mine: (u32, u32),
    root: bool,
) -> (Option<u32>, Option<u32>) {
    match (root, owner == mine) {
        (true, true) => (None, None),
        (true, false) => (Some(owner.0), Some(owner.1)),
        (false, _) => (None, (owner.1 != mine.1).then_some(owner.1)),
    }
}

fn write_temp(
    dir: &OwnedFd,
    temp: &str,
    text: &str,
    mode: u32,
    owner: (u32, u32),
) -> std::io::Result<()> {
    let _ = rustix::fs::unlinkat(dir, temp, AtFlags::empty());
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::from(rustix::fs::openat(
            dir,
            temp,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(mode as _),
        )?);
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        let mine = (
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw(),
        );
        let (uid, gid) = owner_change(owner, mine, rustix::process::geteuid().is_root());
        if uid.is_some() || gid.is_some() {
            std::os::unix::fs::fchown(&f, uid, gid)?;
        }
        rustix::fs::fchmod(&f, Mode::from_bits_truncate(mode as _))?;
        Ok(())
    })();
    if written.is_err() {
        let _ = rustix::fs::unlinkat(dir, temp, AtFlags::empty());
    }
    written
}

pub fn stage_replace(path: &Path, before: &str, text: &str) -> Result<Pending, Refused> {
    let real = std::fs::canonicalize(path)?;
    let (folder, name) = split(&real)?;
    let dir = open_folder(folder)?;
    let stat =
        rustix::fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(Refused::Io(std::io::Error::other(
            "the config is no longer a regular file",
        )));
    }
    if read_at(&dir, name)? != before {
        return Err(Refused::Changed);
    }
    let temp = temp_name(name);
    write_temp(
        &dir,
        &temp,
        text,
        stat.st_mode as u32 & 0o777,
        (stat.st_uid, stat.st_gid),
    )?;
    Ok(Pending {
        dir,
        temp,
        name: name.to_os_string(),
        made: None,
        kind: Kind::Replace,
    })
}

fn make_folder(
    parent: &Path,
    model: &std::fs::Metadata,
) -> Result<Option<std::path::PathBuf>, Refused> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if parent.exists() {
        return Ok(None);
    }
    let holder = parent.parent().unwrap_or(Path::new("."));
    let mode = std::fs::metadata(holder)?.permissions().mode() & 0o750;
    std::fs::create_dir(parent)?;
    let made = std::fs::canonicalize(parent)?;
    let set = std::fs::set_permissions(&made, std::fs::Permissions::from_mode(mode | 0o700));
    let owned = if rustix::process::geteuid().is_root() {
        std::os::unix::fs::chown(&made, Some(model.uid()), Some(model.gid()))
    } else {
        Ok(())
    };
    if let Err(e) = set.and(owned) {
        let _ = std::fs::remove_dir(&made);
        return Err(Refused::Io(e));
    }
    Ok(Some(made))
}

pub fn stage_create(path: &Path, text: &str, like: &Path) -> Result<Pending, Refused> {
    use std::os::unix::fs::MetadataExt;
    let model = std::fs::metadata(like)?;
    let parent = path
        .parent()
        .ok_or(Refused::Io(std::io::ErrorKind::InvalidInput.into()))?;
    let made = make_folder(parent, &model)?;
    let undo_made = |e: Refused| {
        if let Some(d) = &made {
            let _ = std::fs::remove_dir(d);
        }
        e
    };
    let folder = std::fs::canonicalize(parent).map_err(|e| undo_made(e.into()))?;
    let name = path
        .file_name()
        .ok_or(Refused::Io(std::io::ErrorKind::InvalidInput.into()))?;
    let dir = open_folder(&folder).map_err(|e| undo_made(e.into()))?;
    if rustix::fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
        return Err(undo_made(Refused::Changed));
    }
    let temp = temp_name(name);
    write_temp(
        &dir,
        &temp,
        text,
        model.mode() & 0o666,
        (model.uid(), model.gid()),
    )
    .map_err(|e| undo_made(e.into()))?;
    Ok(Pending {
        dir,
        temp,
        name: name.to_os_string(),
        made,
        kind: Kind::Create,
    })
}

pub struct Hidden {
    dir: OwnedFd,
    name: OsString,
    spare: String,
}

impl Hidden {
    pub fn commit(&self) -> std::io::Result<()> {
        rustix::fs::renameat(
            &self.dir,
            self.name.as_os_str(),
            &self.dir,
            self.spare.as_str(),
        )?;
        sync_folder(&self.dir);
        Ok(())
    }

    pub fn undo(&self) -> std::io::Result<()> {
        rustix::fs::renameat(
            &self.dir,
            self.spare.as_str(),
            &self.dir,
            self.name.as_os_str(),
        )?;
        sync_folder(&self.dir);
        Ok(())
    }

    pub fn finish(&self) {
        let _ = rustix::fs::unlinkat(&self.dir, self.spare.as_str(), AtFlags::empty());
    }
}

pub fn stage_remove(path: &Path, before: &str) -> Result<Hidden, Refused> {
    let real = std::fs::canonicalize(path)?;
    let (folder, name) = split(&real)?;
    let dir = open_folder(folder)?;
    let stat =
        rustix::fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(Refused::Io(std::io::Error::other(
            "the file is no longer a regular file",
        )));
    }
    if read_at(&dir, name)? != before {
        return Err(Refused::Changed);
    }
    Ok(Hidden {
        spare: temp_name(name),
        name: name.to_os_string(),
        dir,
    })
}

pub fn open_folder(folder: &Path) -> std::io::Result<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    if !rustix::process::geteuid().is_root() {
        return Ok(rustix::fs::open(folder, flags, Mode::empty())?);
    }
    let mut dir = rustix::fs::openat(rustix::fs::CWD, "/", flags, Mode::empty())?;
    for part in folder.components() {
        if let std::path::Component::Normal(part) = part {
            dir = rustix::fs::openat(&dir, part, flags, Mode::empty())?;
        }
    }
    Ok(dir)
}
