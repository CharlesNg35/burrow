#![cfg(test)]
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

#[test]
fn a_forged_copy_note_can_only_name_files_in_the_folder() {
    let p = place();
    let backend = crate::service::tests::Recorder(Default::default(), false);
    write(
        &p.source,
        &server("  web: { client: home, target: 80, bind: 7900 }\n"),
    );
    let folder = p.config.with_file_name("server.d");
    let outside = p.config.with_file_name("victim.yaml");
    write(&outside, "keep\n");
    write(&folder.join("mine.yaml"), "services: {}\n");
    write(
        &folder.join(COPIED),
        &format!(
            "../victim.yaml\n{}\nmine.yaml/../../victim.yaml\n",
            outside.display()
        ),
    );
    copy(
        &backend,
        (&p.source, &p.config),
        "burrow",
        false,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep\n");
    assert!(folder.join("mine.yaml").exists());
}
