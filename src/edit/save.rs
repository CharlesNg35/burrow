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

pub fn write_back(path: &Path, before: &str, text: &str) -> Result<(), Refused> {
    if std::fs::read_to_string(path)? != before {
        return Err(Refused::Changed);
    }
    replace(path, text)?;
    Ok(())
}

#[cfg(unix)]
fn replace(path: &Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let meta = std::fs::metadata(path)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let next = path.with_file_name(format!(".{name}.burrow-edit-{}", std::process::id()));
    let _ = std::fs::remove_file(&next);
    let written = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(meta.mode() & 0o7777)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&next)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        f.set_permissions(std::fs::Permissions::from_mode(meta.mode() & 0o7777))?;
        let mine = (
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw(),
        );
        if (meta.uid(), meta.gid()) != mine {
            std::os::unix::fs::fchown(&f, Some(meta.uid()), Some(meta.gid()))?;
        }
        std::fs::rename(&next, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&next);
    }
    written
}

#[cfg(not(unix))]
fn replace(path: &Path, text: &str) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

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
