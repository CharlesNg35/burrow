use std::io::Write;
use std::path::Path;

use super::Backend;
use super::system;
use crate::base::failure::Failure;
use crate::base::output::say;
use crate::config::docs::{self, Doc, Docs, ReadOnly, drop_in_folder};

fn file_part(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn listed(main: &Path) -> Result<Vec<String>, Failure> {
    docs::list(main)
        .map(|l| l.map(|l| l.names).unwrap_or_default())
        .map_err(|e| {
            Failure::new(format!(
                "cannot read {}: {e}",
                drop_in_folder(main).display()
            ))
        })
}

pub(super) fn copy(
    backend: &dyn Backend,
    (source, config): (&Path, &Path),
    account: &str,
    writable: bool,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let (from, to) = (drop_in_folder(source), drop_in_folder(config));
    let names = listed(source)?;
    let wanted: Vec<&str> = names.iter().map(|n| file_part(n)).collect();
    for old in listed(config)? {
        if !wanted.contains(&file_part(&old)) {
            let gone = to.join(file_part(&old));
            std::fs::remove_file(&gone)
                .map_err(|e| Failure::new(format!("cannot remove {}: {e}", gone.display())))?;
            say(
                out,
                format!(
                    "removed {}, which {} no longer has",
                    gone.display(),
                    from.display()
                ),
            )?;
        }
    }
    if names.is_empty() {
        return Ok(());
    }
    backend.config_folder(&to, config, account, writable)?;
    for file in &wanted {
        let (src, dst) = (from.join(file), to.join(file));
        let locked = docs::read_only(&src) == Some(ReadOnly::Marked);
        system::copy_file(&src, &dst, false)?;
        backend.secure_file(&dst, account)?;
        if locked {
            keep_read_only(&dst, account)?;
        }
    }
    say(
        out,
        format!(
            "copied {} -> {} ({} {})",
            from.display(),
            to.display(),
            wanted.len(),
            crate::config::diagnostics::plural(wanted.len(), "file")
        ),
    )
}

#[cfg(unix)]
fn keep_read_only(path: &Path, account: &str) -> Result<(), Failure> {
    use std::os::unix::fs::PermissionsExt;
    let root = rustix::process::geteuid().is_root();
    let gid = root
        .then(|| crate::base::process::account_ids(account).ok())
        .flatten()
        .map(|(_, g)| g);
    let mode = if gid.is_some() { 0o440 } else { 0o400 };
    if root {
        std::os::unix::fs::chown(path, Some(0), gid)
            .map_err(|e| Failure::new(format!("cannot give {} to root: {e}", path.display())))?;
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| Failure::new(format!("cannot set permissions on {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn keep_read_only(path: &Path, _account: &str) -> Result<(), Failure> {
    let mut perms = std::fs::metadata(path)
        .map_err(|e| Failure::new(format!("cannot read {}: {e}", path.display())))?
        .permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(path, perms)
        .map_err(|e| Failure::new(format!("cannot mark {} read-only: {e}", path.display())))
}

pub(super) fn installed(config: &Path, main: String) -> Docs {
    let mut docs = Docs::one(docs::main_name(config), main);
    for name in listed(config).unwrap_or_default() {
        if let Some(text) = system::read_plain_file(&docs::path_of(config, &name))
            .and_then(|b| String::from_utf8(b).ok())
        {
            docs.list.push(Doc { name, text });
        }
    }
    docs
}

pub(super) fn same_set(source: &Path, installed: &Docs) -> bool {
    let Ok(src) = docs::read(source) else {
        return false;
    };
    src.list.len() == installed.list.len()
        && src
            .list
            .iter()
            .zip(&installed.list)
            .enumerate()
            .all(|(i, (a, b))| {
                a.text == b.text && (i == 0 || file_part(&a.name) == file_part(&b.name))
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_only(p: &Path) -> bool {
        std::fs::metadata(p).unwrap().permissions().readonly()
    }

    #[test]
    fn the_folder_is_copied_with_the_config_and_read_only_files_stay_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("mine.yaml");
        std::fs::write(&source, "listen: 1\n").unwrap();
        let from = tmp.path().join("mine.d");
        std::fs::create_dir(&from).unwrap();
        std::fs::write(from.join("apps.yaml"), "services: {}\n").unwrap();
        std::fs::write(from.join("tool.yaml"), "clients: {}\n").unwrap();
        std::fs::write(from.join("notes.txt"), "x").unwrap();
        let mut perms = std::fs::metadata(from.join("tool.yaml"))
            .unwrap()
            .permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(from.join("tool.yaml"), perms).unwrap();
        let folder = tmp.path().join("etc");
        std::fs::create_dir(&folder).unwrap();
        let config = folder.join("server.yaml");
        std::fs::write(&config, "listen: 1\n").unwrap();
        std::fs::create_dir(folder.join("server.d")).unwrap();
        std::fs::write(folder.join("server.d/old.yaml"), "services: {}\n").unwrap();
        let backend = crate::service::tests::Recorder(Default::default(), false);
        let mut out = Vec::new();
        copy(&backend, (&source, &config), "burrow", true, &mut out).unwrap();
        let to = folder.join("server.d");
        assert_eq!(
            std::fs::read_to_string(to.join("apps.yaml")).unwrap(),
            "services: {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(to.join("tool.yaml")).unwrap(),
            "clients: {}\n"
        );
        assert!(!to.join("notes.txt").exists());
        assert!(
            !to.join("old.yaml").exists(),
            "a file the source no longer has is removed"
        );
        assert!(read_only(&to.join("tool.yaml")));
        assert!(!read_only(&to.join("apps.yaml")));
        let said = String::from_utf8(out).unwrap();
        assert!(said.contains("(2 files)"), "{said}");
        assert!(
            backend
                .0
                .borrow()
                .iter()
                .any(|n| n == "folder writable=true burrow"),
            "{:?}",
            backend.0.borrow()
        );
        let docs = installed(&config, "listen: 1\n".into());
        assert_eq!(docs.list.len(), 3);
        assert!(same_set(&source, &docs));
        #[cfg(windows)]
        for p in [from.join("tool.yaml"), to.join("tool.yaml")] {
            let mut perms = std::fs::metadata(&p).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            std::fs::set_permissions(&p, perms).unwrap();
        }
    }
}
