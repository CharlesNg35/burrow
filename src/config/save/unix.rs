use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};

use super::{Refused, spare_prefix};

pub struct Pending {
    dir: OwnedFd,
    temp: String,
    name: OsString,
    made: Option<std::path::PathBuf>,
}

impl Pending {
    pub fn commit(&self) -> std::io::Result<()> {
        let moved = rustix::fs::renameat(
            &self.dir,
            self.temp.as_str(),
            &self.dir,
            self.name.as_os_str(),
        );
        if moved.is_err() {
            self.discard();
        }
        moved?;
        rustix::fs::fsync(&self.dir)?;
        Ok(())
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
        rustix::fs::fchmod(&f, Mode::from_bits_truncate(mode as _))?;
        let mine = (
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw(),
        );
        if owner != mine {
            std::os::unix::fs::fchown(&f, Some(owner.0), Some(owner.1))?;
        }
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
