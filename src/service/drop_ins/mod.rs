use std::io::Write;
use std::path::Path;

use super::Backend;
use super::system;

#[cfg(not(unix))]
mod other;
#[cfg(unix)]
mod unix;

use crate::base::failure::Failure;
use crate::base::output::say;
use crate::config::docs::{self, Doc, Docs, ReadOnly, drop_in_folder};
#[cfg(not(unix))]
use other as place;
#[cfg(unix)]
use unix as place;

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

fn is_link_or_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & REPARSE_POINT != 0 {
            return true;
        }
    }
    !meta.file_type().is_dir()
}

fn refuse_link(to: &Path) -> Result<(), Failure> {
    if is_link_or_file(to) {
        return Err(Failure::new(format!(
            "{} is a link or a file, not a folder, so burrow will not install drop-in files there",
            to.display()
        ))
        .hint("remove it, and run the install again"));
    }
    Ok(())
}

pub(super) fn copy(
    backend: &dyn Backend,
    (source, config): (&Path, &Path),
    account: &str,
    writable: bool,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let (from, to) = (drop_in_folder(source), drop_in_folder(config));
    refuse_link(&to)?;
    let names = listed(source)?;
    let wanted: Vec<&str> = names.iter().map(|n| file_part(n)).collect();
    for old in listed(config)? {
        if !wanted.contains(&file_part(&old)) {
            say(
                out,
                format!(
                    "left {} in place: {} does not have it, and burrow never deletes a drop-in file it did not get from the source",
                    to.join(file_part(&old)).display(),
                    from.display()
                ),
            )?;
        }
    }
    if names.is_empty() {
        return Ok(());
    }
    let folder = place::Folder::open(backend, (config, &to), account, writable)?;
    for file in &wanted {
        let src = from.join(file);
        let text = std::fs::read(&src)
            .map_err(|e| Failure::new(format!("cannot read {}: {e}", src.display())))?;
        let locked = docs::read_only(&src) == Some(ReadOnly::Marked);
        folder.put(file, &text, locked)?;
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

pub(super) fn installed(config: &Path, main: String) -> Docs {
    let mut docs = Docs::one(docs::main_name(config), main);
    if is_link_or_file(&drop_in_folder(config)) {
        return docs;
    }
    for name in listed(config).unwrap_or_default() {
        if let Some(text) = docs::path_of(config, &name)
            .and_then(|p| system::read_plain_file(&p))
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
        copy(
            &backend,
            (&source, &config),
            "burrow",
            true,
            &mut Vec::new(),
        )
        .expect("a second install replaces the read-only copy");
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
        assert_eq!(
            std::fs::read_to_string(to.join("old.yaml")).unwrap(),
            "services: {}\n",
            "a file the source does not have is left in place"
        );
        assert!(read_only(&to.join("tool.yaml")));
        assert!(!read_only(&to.join("apps.yaml")));
        let said = String::from_utf8(out).unwrap();
        assert!(said.contains("(2 files)"), "{said}");
        assert!(
            said.contains("left") && said.contains("old.yaml") && said.contains("in place"),
            "{said}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&to).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "the service may change the folder");
        }
        #[cfg(not(unix))]
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
        assert_eq!(docs.list.len(), 4);
        assert!(
            !same_set(&source, &docs),
            "the kept file makes the sets differ"
        );
        #[cfg(windows)]
        for p in [from.join("tool.yaml"), to.join("tool.yaml")] {
            let mut perms = std::fs::metadata(&p).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            std::fs::set_permissions(&p, perms).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_installed_drop_in_folder_or_file_that_is_a_link_is_never_followed() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("mine.yaml");
        std::fs::write(&source, "listen: 1\n").unwrap();
        std::fs::create_dir(tmp.path().join("mine.d")).unwrap();
        std::fs::write(tmp.path().join("mine.d/apps.yaml"), "services: {}\n").unwrap();
        let victim = tmp.path().join("victim");
        std::fs::create_dir(&victim).unwrap();
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(victim.join("important.yaml"), "keep\n").unwrap();
        std::fs::write(victim.join("apps.yaml"), "victim\n").unwrap();
        let folder = tmp.path().join("etc");
        std::fs::create_dir(&folder).unwrap();
        let config = folder.join("server.yaml");
        std::fs::write(&config, "listen: 1\n").unwrap();
        std::os::unix::fs::symlink(&victim, folder.join("server.d")).unwrap();
        let backend = crate::service::tests::Recorder(Default::default(), false);
        let linked = copy(
            &backend,
            (&source, &config),
            "burrow",
            true,
            &mut Vec::new(),
        );
        assert!(linked.is_err(), "a linked server.d is refused");
        let victim_state = || {
            (
                std::fs::read_to_string(victim.join("important.yaml")).ok(),
                std::fs::read_to_string(victim.join("apps.yaml")).ok(),
                std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
            )
        };
        assert_eq!(
            victim_state(),
            (Some("keep\n".into()), Some("victim\n".into()), 0o755)
        );
        std::fs::remove_file(folder.join("server.d")).unwrap();
        std::fs::create_dir(folder.join("server.d")).unwrap();
        std::os::unix::fs::symlink(victim.join("apps.yaml"), folder.join("server.d/apps.yaml"))
            .unwrap();
        copy(
            &backend,
            (&source, &config),
            "burrow",
            true,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            victim_state(),
            (Some("keep\n".into()), Some("victim\n".into()), 0o755)
        );
        assert!(
            !std::fs::symlink_metadata(folder.join("server.d/apps.yaml"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("server.d/apps.yaml")).unwrap(),
            "services: {}\n"
        );
    }

    #[test]
    fn a_source_without_a_folder_leaves_every_installed_drop_in_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("mine.yaml");
        std::fs::write(&source, "listen: 1\n").unwrap();
        let folder = tmp.path().join("etc");
        std::fs::create_dir_all(folder.join("server.d")).unwrap();
        let config = folder.join("server.yaml");
        std::fs::write(&config, "listen: 1\n").unwrap();
        std::fs::write(folder.join("server.d/team.yaml"), "visitors: {}\n").unwrap();
        let backend = crate::service::tests::Recorder(Default::default(), false);
        let mut out = Vec::new();
        copy(&backend, (&source, &config), "burrow", false, &mut out).unwrap();
        assert!(folder.join("server.d/team.yaml").exists());
        let said = String::from_utf8(out).unwrap();
        assert!(
            said.contains("team.yaml") && said.contains("in place"),
            "{said}"
        );
    }
}
