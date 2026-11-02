use std::collections::BTreeMap;
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

const COPIED: &str = ".burrow-copied";

fn copied_before(to: &Path) -> Vec<String> {
    docs::read_regular(&to.join(COPIED))
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn leftovers(source: &Path, config: &Path) -> Result<Vec<String>, Failure> {
    let to = drop_in_folder(config);
    refuse_link(&to)?;
    let wanted: Vec<String> = listed(source)?
        .iter()
        .map(|n| file_part(n).to_string())
        .collect();
    let before = copied_before(&to);
    Ok(listed(config)?
        .iter()
        .map(|n| file_part(n).to_string())
        .filter(|f| !wanted.contains(f) && !before.contains(f))
        .collect())
}

pub(super) fn check_leftovers(
    source: &Path,
    config: &Path,
    env: &BTreeMap<String, String>,
) -> Result<(), Failure> {
    if crate::base::fs::canonical_or_same(source) == crate::base::fs::canonical_or_same(config) {
        return Ok(());
    }
    let kept = leftovers(source, config)?;
    if kept.is_empty() {
        return Ok(());
    }
    let to = drop_in_folder(config);
    let main = crate::config::load::Input {
        name: source.display().to_string(),
        text: std::fs::read_to_string(source).unwrap_or_default(),
        format: crate::config::Format::from_path(source).unwrap_or(crate::config::Format::Yaml),
    };
    let mut found = Vec::new();
    let mut inputs = crate::config::load::read_set(source, main, &mut found);
    for file in &kept {
        let path = to.join(file);
        if let Ok(text) = docs::read_regular(&path) {
            inputs.push(crate::config::load::Input {
                name: path.display().to_string(),
                text,
                format: crate::config::Format::Yaml,
            });
        }
    }
    let lookup = |k: &str| env.get(k).cloned();
    let dir = docs::folder_of(source);
    let state = crate::install::state_dir_for(config);
    let outcome = crate::config::load::finish(
        crate::config::load::load_set(
            source,
            (&dir, &state),
            inputs,
            Some(crate::config::Role::Server),
            &lookup,
        ),
        found,
    );
    let Some(first) = outcome.report.diagnostics.iter().find(|d| d.is_error()) else {
        return Ok(());
    };
    let shown: Vec<String> = kept
        .iter()
        .map(|f| to.join(f).display().to_string())
        .collect();
    Err(Failure::new(format!(
        "{} {} not from the source and would stay, but with it the installed config is invalid: {}",
        shown.join(", "),
        if shown.len() == 1 { "is" } else { "are" },
        first.message
    ))
    .hint("burrow never deletes a drop-in file it did not copy; remove or fix it by hand, then install again"))
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
    let installed = listed(config)?;
    if names.is_empty() && installed.is_empty() {
        return Ok(());
    }
    let folder = place::Folder::open(backend, (config, &to), account, writable)?;
    let before = copied_before(&to);
    for old in installed.iter().map(|n| file_part(n)) {
        if wanted.contains(&old) {
            continue;
        }
        let gone = to.join(old);
        if before.iter().any(|b| b == old) {
            folder.remove(old)?;
            say(
                out,
                format!(
                    "removed {}, which an earlier install copied and {} no longer has",
                    gone.display(),
                    from.display()
                ),
            )?;
        } else {
            say(
                out,
                format!(
                    "left {} in place: burrow did not copy it, and never deletes a drop-in file it did not copy",
                    gone.display()
                ),
            )?;
        }
    }
    for file in &wanted {
        let src = from.join(file);
        let text = std::fs::read(&src)
            .map_err(|e| Failure::new(format!("cannot read {}: {e}", src.display())))?;
        let locked = docs::read_only(&src) == Some(ReadOnly::Marked);
        folder.put(file, &text, locked)?;
    }
    folder.put(COPIED, wanted.join("\n").as_bytes(), false)?;
    if wanted.is_empty() {
        return Ok(());
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

    const SECRET: &str = "yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=";

    fn server(services: &str) -> String {
        format!(
            "listen: 127.0.0.1:7835\nclients:\n  home: {{ secret: \"{SECRET}\" }}\nservices:\n{services}"
        )
    }

    struct Place {
        _tmp: tempfile::TempDir,
        source: std::path::PathBuf,
        config: std::path::PathBuf,
    }

    fn place() -> Place {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("mine.yaml");
        let folder = tmp.path().join("etc");
        std::fs::create_dir(&folder).unwrap();
        let config = folder.join("server.yaml");
        Place {
            _tmp: tmp,
            source,
            config,
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        crate::base::fs::write_private(path, text).unwrap();
    }

    #[test]
    fn a_service_moved_back_into_the_main_file_removes_the_file_install_copied_before() {
        let p = place();
        let backend = crate::service::tests::Recorder(Default::default(), false);
        let blog = "services:\n  blog: { client: home, target: 81, bind: 7901 }\n";
        write(
            &p.source,
            &server("  web: { client: home, target: 80, bind: 7900 }\n"),
        );
        write(&p.source.with_file_name("mine.d/blog.yaml"), blog);
        let none = BTreeMap::new();
        check_leftovers(&p.source, &p.config, &none).unwrap();
        copy(
            &backend,
            (&p.source, &p.config),
            "burrow",
            false,
            &mut Vec::new(),
        )
        .unwrap();
        let installed = p.config.with_file_name("server.d/blog.yaml");
        assert!(installed.exists());
        std::fs::remove_file(p.source.with_file_name("mine.d/blog.yaml")).unwrap();
        write(
            &p.source,
            &server(
                "  web: { client: home, target: 80, bind: 7900 }\n  blog: { client: home, target: 81, bind: 7901 }\n",
            ),
        );
        check_leftovers(&p.source, &p.config, &none).unwrap();
        let mut out = Vec::new();
        copy(&backend, (&p.source, &p.config), "burrow", false, &mut out).unwrap();
        assert!(
            !installed.exists(),
            "the copied file the source dropped is removed"
        );
        let said = String::from_utf8(out).unwrap();
        assert!(
            said.contains("removed") && said.contains("blog.yaml"),
            "{said}"
        );
    }

    #[test]
    fn a_hand_added_drop_in_that_clashes_with_the_source_refuses_the_install() {
        let p = place();
        write(
            &p.source,
            &server("  web: { client: home, target: 80, bind: 7900 }\n"),
        );
        let mine = p.config.with_file_name("server.d/mine.yaml");
        write(
            &mine,
            "services:\n  web: { client: home, target: 82, bind: 7902 }\n",
        );
        let e = check_leftovers(&p.source, &p.config, &BTreeMap::new()).expect_err("refused");
        assert!(e.message.contains("mine.yaml"), "{}", e.message);
        assert!(e.message.contains("defined twice"), "{}", e.message);
        assert!(mine.exists());
    }
}
