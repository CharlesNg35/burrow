use std::path::{Path, PathBuf};

use crate::config::env::EnvSource;
use crate::config::load::{Input, load_with_drop_ins};
use crate::config::{Format, Outcome, Role};

pub fn check(
    original: &Path,
    format: Format,
    role: Role,
    text: &str,
    env: &dyn EnvSource,
) -> Outcome {
    let dir = original.parent().unwrap_or(Path::new("."));
    let state = crate::install::state_dir_for(original);
    let main = Input {
        name: original.display().to_string(),
        text: text.to_string(),
        format,
    };
    load_with_drop_ins(
        original,
        (dir, &state),
        vec![main],
        Some(role),
        role == Role::Server,
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

#[cfg(not(unix))]
mod other;
mod set;
#[cfg(unix)]
mod unix;

#[cfg(not(unix))]
use other as platform;
#[cfg(unix)]
use unix as platform;

pub use set::{changes, write_set};

pub fn write_back(path: &Path, before: &str, text: &str) -> Result<(), Refused> {
    Ok(platform::stage_replace(path, before, text)?.commit()?)
}

fn spare_prefix(name: &str) -> String {
    format!(".{name}.burrow-edit-")
}

pub fn history_dir_of(config: &Path) -> PathBuf {
    let name = config
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".into());
    config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        .join(format!(".{name}.history"))
}

pub fn history_upload_ids(history: &Path) -> Vec<String> {
    use std::io::Read;
    let Ok(entries) = std::fs::read_dir(history) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let Some(mut file) = crate::base::fs::open_plain(&entry.path()).ok().flatten() else {
            continue;
        };
        let mut text = String::new();
        if file.read_to_string(&mut text).is_err() {
            continue;
        }
        for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            if burrow_syntax::upload::kind_of(word).is_some() && !ids.iter().any(|i| i == word) {
                ids.push(word.to_string());
            }
        }
    }
    ids
}

pub fn spare_next_to(path: &Path) -> Option<std::path::PathBuf> {
    let real = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leftover_spare_is_found_next_to_its_config() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = dunce::canonicalize(tmp.path()).unwrap();
        let config = dir.join("server.yaml");
        std::fs::write(&config, "x").unwrap();
        assert_eq!(spare_next_to(&config), None);
        std::fs::write(dir.join(".other.yaml.burrow-edit-7"), "x").unwrap();
        assert_eq!(spare_next_to(&config), None);
        let spare = dir.join(".server.yaml.burrow-edit-42");
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
            Some(dunce::canonicalize(&spare).unwrap())
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
        assert!(unix::open_folder(&real).is_ok());
        let through = std::fs::canonicalize(base.path()).unwrap().join("link");
        assert!(unix::open_folder(&through).is_err());
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
        crate::base::fs::write_private(&path, "name: home\n").unwrap();
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
        let env = |k: &str| {
            (k == "HOME_SECRET").then(|| crate::wire::secret::Secret::generate().encode())
        };
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
