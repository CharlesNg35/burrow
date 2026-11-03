use std::io::Write;
use std::path::Path;

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, Mode, OFlags};

use super::Put;
use crate::base::failure::Failure;
use crate::service::Backend;
use crate::service::nofollow::{failed, made_subfolder};

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
        let dir = made_subfolder(parent, to.file_name().unwrap_or_default())?;
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

    #[test]
    fn a_drop_in_folder_that_is_a_link_is_refused_and_its_target_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("server.yaml");
        let target = tmp.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let to = tmp.path().join("server.d");
        std::os::unix::fs::symlink(&target, &to).unwrap();
        let backend = crate::service::tests::Recorder(Default::default(), false);
        let Err(err) = Folder::open(&backend, (&config, &to), "burrow", true) else {
            panic!("a linked server.d is opened");
        };
        assert!(err.message.contains("not a link"), "{}", err.message);
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "the link's target keeps its mode");
        std::fs::remove_file(&to).unwrap();
        let folder = Folder::open(&backend, (&config, &to), "burrow", true).unwrap();
        folder
            .put("apps.yaml", b"services: {}\n", Put::Config)
            .unwrap();
        assert!(to.join("apps.yaml").is_file());
        let mode = std::fs::metadata(&to).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "a writable install keeps the folder private"
        );
    }
}
