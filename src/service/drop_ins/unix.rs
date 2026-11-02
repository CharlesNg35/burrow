use std::io::Write;
use std::path::Path;

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, Mode, OFlags};

use super::Put;
use crate::base::failure::Failure;
use crate::service::Backend;

const FOLDER: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

pub(super) struct Folder {
    dir: OwnedFd,
    shown: std::path::PathBuf,
    owner: Option<(u32, u32)>,
    group: Option<u32>,
    writable: bool,
}

fn owner_and_mode(
    put: Put,
    (owner, group): (Option<(u32, u32)>, Option<u32>),
    writable: bool,
) -> (Option<(u32, u32)>, rustix::fs::RawMode) {
    match put {
        Put::Locked if owner.is_some() => (Some((0, group.unwrap_or(0))), 0o440),
        Put::Locked => (None, 0o400),
        Put::Note if !writable => (owner.map(|_| (0, 0)), 0o644),
        Put::Note | Put::Config => (owner, 0o600),
    }
}

fn failed(what: &str, path: &Path, e: impl std::fmt::Display) -> Failure {
    Failure::new(format!("cannot {what} {}: {e}", path.display()))
}

impl Folder {
    pub(super) fn open(
        _backend: &dyn Backend,
        (config, to): (&Path, &Path),
        account: &str,
        writable: bool,
    ) -> Result<Folder, Failure> {
        let parent = config
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = to.file_name().unwrap_or_default();
        let holder = rustix::fs::open(parent, FOLDER, Mode::empty())
            .map_err(|e| failed("open", parent, e))?;
        if let Err(e) = rustix::fs::mkdirat(&holder, name, Mode::from_bits_truncate(0o700))
            && e != rustix::io::Errno::EXIST
        {
            return Err(failed("create", to, e));
        }
        let dir = rustix::fs::openat(&holder, name, FOLDER, Mode::empty())
            .map_err(|e| failed("open", to, format!("{e}; it must be a folder, not a link")))?;
        let root = rustix::process::geteuid().is_root();
        let ids = root
            .then(|| crate::base::process::account_ids(account).ok())
            .flatten();
        let owner = root.then(|| {
            if writable {
                ids.unwrap_or((0, 0))
            } else {
                (0, 0)
            }
        });
        if let Some((uid, gid)) = owner {
            std::os::unix::fs::fchown(&dir, Some(uid), Some(gid))
                .map_err(|e| failed("give", to, e))?;
        }
        let mode = if writable { 0o700 } else { 0o755 };
        rustix::fs::fchmod(&dir, Mode::from_bits_truncate(mode))
            .map_err(|e| failed("set permissions on", to, e))?;
        Ok(Folder {
            dir,
            shown: to.to_path_buf(),
            owner: root.then(|| ids.unwrap_or((0, 0))),
            group: ids.map(|(_, g)| g),
            writable,
        })
    }

    pub(super) fn put(&self, file: &str, text: &[u8], put: Put) -> Result<(), Failure> {
        let shown = self.shown.join(file);
        let temp = format!(".{file}.burrow-new");
        let _ = rustix::fs::unlinkat(&self.dir, temp.as_str(), AtFlags::empty());
        let (owner, mode) = owner_and_mode(put, (self.owner, self.group), self.writable);
        let written = (|| -> std::io::Result<()> {
            let mut f = std::fs::File::from(rustix::fs::openat(
                &self.dir,
                temp.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            f.write_all(text)?;
            f.sync_all()?;
            if let Some((uid, gid)) = owner {
                std::os::unix::fs::fchown(&f, Some(uid), Some(gid))?;
            }
            rustix::fs::fchmod(&f, Mode::from_bits_truncate(mode))?;
            rustix::fs::renameat(&self.dir, temp.as_str(), &self.dir, file)?;
            Ok(())
        })();
        if let Err(e) = written {
            let _ = rustix::fs::unlinkat(&self.dir, temp.as_str(), AtFlags::empty());
            return Err(failed("write", &shown, e));
        }
        Ok(())
    }

    pub(super) fn remove(&self, file: &str) -> Result<(), Failure> {
        rustix::fs::unlinkat(&self.dir, file, AtFlags::empty())
            .map_err(|e| failed("remove", &self.shown.join(file), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_note_belongs_to_root_unless_the_service_may_change_its_config() {
        let account = Some((1000, 1001));
        assert_eq!(
            owner_and_mode(Put::Note, (account, Some(1001)), false),
            (Some((0, 0)), 0o644)
        );
        assert_eq!(
            owner_and_mode(Put::Note, (account, Some(1001)), true),
            (account, 0o600)
        );
        assert_eq!(
            owner_and_mode(Put::Config, (account, Some(1001)), false),
            (account, 0o600)
        );
        assert_eq!(
            owner_and_mode(Put::Locked, (account, Some(1001)), false),
            (Some((0, 1001)), 0o440)
        );
        assert_eq!(
            owner_and_mode(Put::Note, (None, None), false),
            (None, 0o644)
        );
    }
}
