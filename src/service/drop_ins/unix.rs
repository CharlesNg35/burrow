use std::io::Write;
use std::path::Path;

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};

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
        let folder = Folder::of(dir, to, account, writable);
        folder.settle_dir()?;
        Ok(folder)
    }

    fn of(dir: OwnedFd, to: &Path, account: &str, writable: bool) -> Folder {
        let root = rustix::process::geteuid().is_root();
        let ids = root
            .then(|| crate::base::process::account_ids(account).ok())
            .flatten();
        Folder {
            dir,
            shown: to.to_path_buf(),
            owner: root.then(|| ids.unwrap_or((0, 0))),
            group: ids.map(|(_, g)| g),
            writable,
        }
    }

    fn settle_dir(&self) -> Result<(), Failure> {
        let to = &self.shown;
        let owner = self
            .owner
            .map(|ids| if self.writable { ids } else { (0, 0) });
        if let Some((uid, gid)) = owner {
            std::os::unix::fs::fchown(&self.dir, Some(uid), Some(gid))
                .map_err(|e| failed("give", to, e))?;
        }
        let mode = if self.writable { 0o700 } else { 0o755 };
        rustix::fs::fchmod(&self.dir, Mode::from_bits_truncate(mode))
            .map_err(|e| failed("set permissions on", to, e))
    }

    fn settle_entry(&self, name: &str, put: Put) -> Result<(), Failure> {
        let shown = self.shown.join(name);
        let Some(f) = plain_at(&self.dir, name) else {
            return Ok(());
        };
        let (owner, mode) = owner_and_mode(put, (self.owner, self.group), self.writable);
        if let Some((uid, gid)) = owner {
            std::os::unix::fs::fchown(&f, Some(uid), Some(gid))
                .map_err(|e| failed("give", &shown, e))?;
        }
        rustix::fs::fchmod(&f, Mode::from_bits_truncate(mode))
            .map_err(|e| failed("set permissions on", &shown, e))
    }

    pub(super) fn put(&self, file: &str, text: &[u8], put: Put) -> Result<(), Failure> {
        let (owner, mode) = owner_and_mode(put, (self.owner, self.group), self.writable);
        self.write(file, text, (owner, Mode::from_bits_truncate(mode)))
    }

    pub(super) fn put_exact(
        &self,
        file: &str,
        text: &[u8],
        (uid, gid, mode): (u32, u32, Mode),
    ) -> Result<(), Failure> {
        self.write(file, text, (Some((uid, gid)), mode))
    }

    fn write(
        &self,
        file: &str,
        text: &[u8],
        (owner, mode): (Option<(u32, u32)>, Mode),
    ) -> Result<(), Failure> {
        let shown = self.shown.join(file);
        let temp = format!(".{file}.burrow-new");
        let _ = rustix::fs::unlinkat(&self.dir, temp.as_str(), AtFlags::empty());
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
            rustix::fs::fchmod(&f, mode)?;
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

pub(in crate::service) fn settle(
    config: &Path,
    account: &str,
    writable: bool,
) -> Result<(), Failure> {
    let to = super::drop_in_folder(config);
    let Ok(dir) = rustix::fs::open(&to, crate::base::fs::NO_FOLLOW_FOLDER, Mode::empty()) else {
        return Ok(());
    };
    let folder = Folder::of(dir, &to, account, writable);
    folder.settle_dir()?;
    folder.settle_entry(super::COPIED, Put::Note)
}

pub(super) fn plain_at(dir: &OwnedFd, name: &str) -> Option<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    if name.is_empty() || name == ".." || name.contains('/') {
        return None;
    }
    let f = rustix::fs::openat(dir, name, flags, Mode::empty()).ok()?;
    let st = rustix::fs::fstat(&f).ok()?;
    let plain = FileType::from_raw_mode(st.st_mode) == FileType::RegularFile && st.st_nlink == 1;
    plain.then_some(f)
}

pub(super) fn owners(config: &Path) -> Vec<(String, (u32, u32))> {
    let mut found = Vec::new();
    let dir =
        crate::service::nofollow::plain_files(&super::drop_in_folder(config), |name, _, st| {
            if let Ok(name) = name.to_str() {
                found.push((name.to_string(), (st.st_uid, st.st_gid)));
            }
        });
    if let Some(st) = dir.and_then(|d| rustix::fs::fstat(&d).ok()) {
        found.push((".".to_string(), (st.st_uid, st.st_gid)));
    }
    found
}

pub(super) fn give_owners(config: &Path, owners: &[(String, (u32, u32))]) {
    let to = super::drop_in_folder(config);
    let Ok(dir) = rustix::fs::open(&to, crate::base::fs::NO_FOLLOW_FOLDER, Mode::empty()) else {
        return;
    };
    for (name, (uid, gid)) in owners {
        let f = match name.as_str() {
            "." => rustix::io::dup(&dir).ok(),
            _ => plain_at(&dir, name),
        };
        if let Some(f) = f
            && rustix::fs::fstat(&f).is_ok_and(|st| (st.st_uid, st.st_gid) != (*uid, *gid))
        {
            let _ = std::os::unix::fs::fchown(&f, Some(*uid), Some(*gid));
        }
    }
}

pub(super) fn copied_names(dir: &OwnedFd) -> Option<Vec<String>> {
    let mut text = String::new();
    let f = plain_at(dir, super::COPIED)
        .filter(|f| rustix::fs::fstat(f).is_ok_and(|st| st.st_uid == 0))?;
    std::io::Read::read_to_string(&mut std::fs::File::from(f), &mut text).ok()?;
    Some(
        text.lines()
            .filter(|n| !n.starts_with('.'))
            .map(str::to_string)
            .collect(),
    )
}

pub(in crate::service) fn give_copied(config: &Path, account: &str, writable: bool) {
    let to = super::drop_in_folder(config);
    let Ok(dir) = rustix::fs::open(&to, crate::base::fs::NO_FOLLOW_FOLDER, Mode::empty()) else {
        return;
    };
    let Some(names) = copied_names(&dir) else {
        return;
    };
    let folder = Folder::of(dir, &to, account, writable);
    for name in &names {
        let put = match crate::config::docs::read_only(&to.join(name)) {
            Some(_) => Put::Locked,
            None => Put::Config,
        };
        let _ = folder.settle_entry(name, put);
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
    fn settling_gives_the_folder_and_the_copy_note_what_a_fresh_install_with_the_setting_gives() {
        use std::os::unix::fs::{MetadataExt, chown};
        if !rustix::process::geteuid().is_root() {
            return;
        }
        let Ok(ids) = crate::base::process::account_ids("nobody") else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("server.yaml");
        let (to, note) = (
            tmp.path().join("server.d"),
            tmp.path().join("server.d/.burrow-copied"),
        );
        std::fs::create_dir(&to).unwrap();
        std::fs::write(&note, "apps.yaml").unwrap();
        let state = |p: &Path| {
            let meta = std::fs::symlink_metadata(p).unwrap();
            ((meta.uid(), meta.gid()), meta.mode() & 0o777)
        };
        for writable in [false, true, false] {
            let backend = crate::service::tests::Recorder(Default::default(), false);
            let fresh = tmp.path().join(format!("fresh-{writable}/server.yaml"));
            std::fs::create_dir_all(fresh.parent().unwrap()).unwrap();
            let made = Folder::open(
                &backend,
                (&fresh, &fresh.with_file_name("server.d")),
                "nobody",
                writable,
            )
            .unwrap();
            made.put(super::super::COPIED, b"apps.yaml", Put::Note)
                .unwrap();
            let want = fresh.with_file_name("server.d");
            chown(&to, Some(if writable { 0 } else { ids.0 }), Some(0)).unwrap();
            settle(&config, "nobody", writable).unwrap();
            assert_eq!(state(&to), state(&want), "writable={writable}");
            assert_eq!(
                state(&note),
                state(&want.join(".burrow-copied")),
                "writable={writable}"
            );
        }
        settle(&tmp.path().join("none/server.yaml"), "nobody", true).unwrap();
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
