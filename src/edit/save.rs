use std::io::Write;
use std::path::Path;

use crate::config::env::EnvSource;
use crate::config::load::load_str_in;
use crate::config::{Format, Outcome, Role};

pub fn check(
    original: &Path,
    format: Format,
    role: Role,
    text: &str,
    env: &dyn EnvSource,
) -> Outcome {
    let dir = original.parent().unwrap_or(Path::new("."));
    load_str_in(
        dir,
        &original.display().to_string(),
        text,
        format,
        Some(role),
        env,
    )
}

#[derive(Debug)]
pub enum Refused {
    Changed,
    Io(std::io::Error),
}

impl From<std::io::Error> for Refused {
    fn from(e: std::io::Error) -> Self {
        Refused::Io(e)
    }
}

#[cfg(unix)]
pub fn write_back(path: &Path, before: &str, text: &str) -> Result<(), Refused> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    use std::io::Read;
    let real = std::fs::canonicalize(path)?;
    let (Some(folder), Some(name)) = (real.parent(), real.file_name()) else {
        return Err(Refused::Io(std::io::ErrorKind::InvalidInput.into()));
    };
    let dir = open_folder(folder)?;
    let stat =
        rustix::fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(Refused::Io(std::io::Error::other(
            "the config is no longer a regular file",
        )));
    }
    let mut now = String::new();
    std::fs::File::from(
        rustix::fs::openat(
            &dir,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    )
    .read_to_string(&mut now)?;
    if now != before {
        return Err(Refused::Changed);
    }
    let mode = stat.st_mode as u32 & 0o777;
    let next = format!(
        "{}{}",
        spare_prefix(&name.to_string_lossy()),
        std::process::id()
    );
    let _ = rustix::fs::unlinkat(&dir, next.as_str(), AtFlags::empty());
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::from(rustix::fs::openat(
            &dir,
            next.as_str(),
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
        if (stat.st_uid, stat.st_gid) != mine {
            std::os::unix::fs::fchown(&f, Some(stat.st_uid), Some(stat.st_gid))?;
        }
        rustix::fs::renameat(&dir, next.as_str(), &dir, name)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = rustix::fs::unlinkat(&dir, next.as_str(), AtFlags::empty());
    }
    Ok(written?)
}

#[cfg(unix)]
fn open_folder(folder: &Path) -> std::io::Result<rustix::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags};
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

fn spare_prefix(name: &str) -> String {
    format!(".{name}.burrow-edit-")
}

pub fn spare_next_to(path: &Path) -> Option<std::path::PathBuf> {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let name = real.file_name()?.to_str()?;
    let folder = real
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let prefix = spare_prefix(name);
    std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix))
        })
}

#[cfg(not(unix))]
pub fn write_back(path: &Path, before: &str, text: &str) -> Result<(), Refused> {
    if std::fs::read_to_string(path)? != before {
        return Err(Refused::Changed);
    }
    replace(path, text)?;
    Ok(())
}

#[cfg(not(unix))]
fn replace(path: &Path, text: &str) -> std::io::Result<()> {
    let real = std::fs::canonicalize(path)?;
    let (Some(folder), Some(name)) = (real.parent(), real.file_name()) else {
        return Err(std::io::ErrorKind::InvalidInput.into());
    };
    let spare = folder.join(format!(
        "{}{}",
        spare_prefix(&name.to_string_lossy()),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&spare);
    let mut copy = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&spare)?;
    copy.write_all(text.as_bytes())?;
    copy.sync_all()?;
    drop(copy);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&real)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::remove_file(&spare)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leftover_spare_is_found_next_to_its_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("server.yaml");
        std::fs::write(&config, "x").unwrap();
        assert_eq!(spare_next_to(&config), None);
        std::fs::write(dir.path().join(".other.yaml.burrow-edit-7"), "x").unwrap();
        assert_eq!(spare_next_to(&config), None);
        let spare = dir.path().join(".server.yaml.burrow-edit-42");
        std::fs::write(&spare, "listen: 7835\n").unwrap();
        assert_eq!(spare_next_to(&config), Some(spare));
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_config_finds_the_spare_next_to_the_file_it_points_to() {
        let base = tempfile::tempdir().unwrap();
        let real = base.path().join("real");
        let links = base.path().join("links");
        std::fs::create_dir(&real).unwrap();
        std::fs::create_dir(&links).unwrap();
        let target = real.join("server.yaml");
        std::fs::write(&target, "x").unwrap();
        let link = links.join("current.yaml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(spare_next_to(&link), None);
        let spare = real.join(format!("{}42", spare_prefix("server.yaml")));
        std::fs::write(&spare, "listen: 7835\n").unwrap();
        assert_eq!(
            spare_next_to(&link),
            Some(std::fs::canonicalize(&spare).unwrap())
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_save_rewrites_the_same_file_so_its_acl_and_owner_stay() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("server.yaml");
        std::fs::write(&config, "listen: 1\n").unwrap();
        let born = std::fs::metadata(&config).unwrap().created().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        write_back(&config, "listen: 1\n", "listen: 2\n").unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "listen: 2\n");
        assert_eq!(std::fs::metadata(&config).unwrap().created().unwrap(), born);
        assert_eq!(
            spare_next_to(&config),
            None,
            "the spare is removed after a good save"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_reached_through_a_link_is_not_opened_for_writing() {
        let base = tempfile::tempdir().unwrap();
        let real = base.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let real = std::fs::canonicalize(&real).unwrap();
        assert!(open_folder(&real).is_ok());
        let through = std::fs::canonicalize(base.path()).unwrap().join("link");
        assert!(open_folder(&through).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_config_under_a_folder_that_can_only_be_traversed_is_saved() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let base = tempfile::tempdir().unwrap();
        let inner = base.path().join("inner");
        std::fs::create_dir(&inner).unwrap();
        let path = inner.join("client.yaml");
        std::fs::write(&path, "name: home\n").unwrap();
        std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o311)).unwrap();
        let saved = write_back(&path, "name: home\n", "name: office\n");
        std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(saved.is_ok(), "{saved:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "name: office\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_config_keeps_its_link_and_loses_setuid_bits() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let target = base.path().join("client.yaml");
        std::fs::write(&target, "name: home\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o4640)).unwrap();
        let link = base.path().join("current.yaml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write_back(&link, "name: home\n", "name: office\n").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "name: office\n");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o640);
    }

    #[test]
    fn the_file_is_replaced_with_its_permissions_and_only_if_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.yaml");
        crate::instance::write_private(&path, "name: home\n").unwrap();
        write_back(&path, "name: home\n", "name: office\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "name: office\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(matches!(
            write_back(&path, "name: home\n", "name: lab\n"),
            Err(Refused::Changed)
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "name: office\n");
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(
            left.len(),
            1,
            "no temporary file is left next to the config"
        );
    }

    #[test]
    fn the_edited_text_is_checked_like_validate_does() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.yaml");
        let nothing = |_: &str| None;
        let bad = check(&path, Format::Yaml, Role::Client, "nmae: home\n", &nothing);
        assert!(bad.report.has_errors());
        let env =
            |k: &str| (k == "HOME_SECRET").then(|| crate::secret::Secret::generate().encode());
        let good = check(
            &path,
            Format::Yaml,
            Role::Client,
            "server: example.com\nname: home\nsecret: ${HOME_SECRET}\nallow: [127.0.0.0/8]\n",
            &env,
        );
        assert!(!good.report.has_errors(), "{:?}", good.report);
    }
}
