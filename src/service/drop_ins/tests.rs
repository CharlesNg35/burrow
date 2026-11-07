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

#[cfg(unix)]
#[test]
fn a_linked_source_file_is_installed_read_only_with_its_content() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("mine.yaml");
    std::fs::write(&source, "listen: 1\n").unwrap();
    let from = tmp.path().join("mine.d");
    std::fs::create_dir(&from).unwrap();
    let kept = tmp.path().join("kept.yaml");
    std::fs::write(&kept, "services: {}\n").unwrap();
    std::os::unix::fs::symlink(&kept, from.join("linked.yaml")).unwrap();
    let shared = tmp.path().join("shared.yaml");
    std::fs::write(&shared, "visitors: {}\n").unwrap();
    std::fs::hard_link(&shared, from.join("twice.yaml")).unwrap();
    std::fs::write(from.join("own.yaml"), "clients: {}\n").unwrap();
    let folder = tmp.path().join("etc");
    std::fs::create_dir(&folder).unwrap();
    let config = folder.join("server.yaml");
    std::fs::write(&config, "listen: 1\n").unwrap();
    let backend = crate::service::tests::Recorder(Default::default(), false);
    copy(
        &backend,
        (&source, &config),
        "burrow",
        true,
        &mut Vec::new(),
    )
    .unwrap();
    let to = folder.join("server.d");
    let linked = to.join("linked.yaml");
    assert!(!std::fs::symlink_metadata(&linked).unwrap().is_symlink());
    assert_eq!(std::fs::read_to_string(&linked).unwrap(), "services: {}\n");
    assert!(read_only(&linked), "a linked source stays read-only");
    assert!(
        read_only(&to.join("twice.yaml")),
        "a hard-linked source too"
    );
    assert!(!read_only(&to.join("own.yaml")));
}

#[cfg(unix)]
#[test]
fn a_copy_that_fails_part_way_puts_the_installed_folder_back() {
    let p = place();
    let backend = crate::service::tests::Recorder(Default::default(), false);
    write(&p.source, &server(""));
    write(&p.source.with_file_name("mine.d/a.yaml"), "services: {}\n");
    write(&p.source.with_file_name("mine.d/b.yaml"), "clients: {}\n");
    let to = p.config.with_file_name("server.d");
    write(&to.join("a.yaml"), "# the old a\n");
    write(&to.join("old.yaml"), "# copied before\n");
    write(&to.join(COPIED), "old.yaml");
    write(&to.join("b.yaml").join("inside"), "a folder with a file\n");
    let e = copy(
        &backend,
        (&p.source, &p.config),
        "burrow",
        true,
        &mut Vec::new(),
    )
    .expect_err("b.yaml cannot replace a folder that holds a file");
    assert!(e.message.contains("b.yaml"), "{}", e.message);
    let read = |f: &str| std::fs::read_to_string(to.join(f)).unwrap();
    assert_eq!(read("a.yaml"), "# the old a\n");
    assert_eq!(read("old.yaml"), "# copied before\n");
    assert_eq!(read(COPIED), "old.yaml");
}

#[test]
fn a_reinstall_put_back_restores_the_drop_ins_it_found() {
    let p = place();
    let blog = "  blog: { client: home, target: 81, bind: 7901 }\n";
    write(&p.config, &server(blog));
    let to = p.config.with_file_name("server.d");
    write(&to.join("apps.yaml"), "# kept by hand\nservices: {}\n");
    let old = crate::install::Manifest {
        config: p.config.clone(),
        env_file: None,
        ..crate::service::tests::record(crate::install::Platform::Linux)
    };
    let undo = crate::service::previous::Undo {
        manifest: old.clone(),
        config: crate::service::system::read_plain_file(&p.config),
        drop_ins: Snapshot::take(&p.config),
        env: BTreeMap::new(),
        binary: None,
    };
    write(&p.config, &server(""));
    write(&to.join("apps.yaml"), &format!("services:\n{blog}"));
    write(&to.join("more.yaml"), "clients: {}\n");
    write(&to.join(COPIED), "apps.yaml\nmore.yaml");
    let f = undo.put_back(
        &crate::service::tests::Recorder(Default::default(), true),
        &old,
        "exited with status 3",
        |_| Ok(()),
        &mut Vec::new(),
    );
    assert!(f.message.contains("runs again"), "{}", f.message);
    assert_eq!(std::fs::read_to_string(&p.config).unwrap(), server(blog));
    assert_eq!(
        std::fs::read_to_string(to.join("apps.yaml")).unwrap(),
        "# kept by hand\nservices: {}\n"
    );
    assert!(!to.join("more.yaml").exists());
    assert!(!to.join(COPIED).exists());
}

#[cfg(unix)]
#[test]
fn a_part_way_copy_over_a_stopped_install_puts_its_config_and_drop_ins_back() {
    let p = place();
    let backend = crate::service::tests::Recorder(Default::default(), false);
    let blog = "  blog: { client: home, target: 81, bind: 7901 }\n";
    write(&p.config, &server(blog));
    let to = p.config.with_file_name("server.d");
    write(&to.join("a.yaml"), "# the old a\n");
    write(&p.source, &server(""));
    write(&p.source.with_file_name("mine.d/a.yaml"), "services: {}\n");
    write(&p.source.with_file_name("mine.d/b.yaml"), "clients: {}\n");
    write(&to.join("b.yaml").join("inside"), "a folder with a file\n");
    let old = crate::install::Manifest {
        config: p.config.clone(),
        env_file: None,
        ..crate::service::tests::record(crate::install::Platform::Linux)
    };
    let undo = crate::service::previous::Undo {
        manifest: old,
        config: crate::service::system::read_plain_file(&p.config),
        drop_ins: Snapshot::take(&p.config),
        env: BTreeMap::new(),
        binary: None,
    };
    let copied = crate::service::install::copy_config(
        &backend,
        (&p.source, &p.config),
        ("burrow", false),
        &mut Vec::new(),
    )
    .inspect_err(|_| {
        let _ = undo.restore_files(&backend);
    });
    assert!(
        copied.is_err(),
        "b.yaml cannot replace a folder that holds a file"
    );
    assert_eq!(std::fs::read_to_string(&p.config).unwrap(), server(blog));
    assert_eq!(
        std::fs::read_to_string(to.join("a.yaml")).unwrap(),
        "# the old a\n"
    );
}

#[cfg(unix)]
fn owner(path: &Path) -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).unwrap();
    (meta.uid(), meta.gid())
}

#[cfg(unix)]
fn account_drop_ins(p: &Place) -> Option<((u32, u32), std::path::PathBuf)> {
    if !rustix::process::geteuid().is_root() {
        return None;
    }
    let ids = crate::base::process::account_ids("nobody").ok()?;
    let to = p.config.with_file_name("server.d");
    write(&p.config, "listen: 1\n");
    write(&to.join("apps.yaml"), "services: {}\n");
    write(&to.join("locked.yaml"), "clients: {}\n");
    let shared = p.config.with_file_name("shared.yaml");
    write(&shared, "visitors: {}\n");
    std::fs::hard_link(&shared, to.join("twice.yaml")).unwrap();
    for path in [to.join("apps.yaml"), shared, to.clone()] {
        std::os::unix::fs::chown(path, Some(ids.0), Some(ids.1)).unwrap();
    }
    std::os::unix::fs::chown(to.join("locked.yaml"), Some(0), Some(ids.1)).unwrap();
    Some((ids, to))
}

#[cfg(unix)]
fn nobody(config: &Path) -> crate::install::Manifest {
    crate::install::Manifest {
        config: config.to_path_buf(),
        account: "nobody".into(),
        env_file: None,
        log_file: config.with_file_name("logs/server.log"),
        copied_binary: false,
        created_account: false,
        ..crate::service::tests::record(crate::install::Platform::Linux)
    }
}

#[cfg(unix)]
#[test]
fn an_uninstall_that_keeps_the_config_gives_the_account_drop_ins_to_root() {
    let p = place();
    let Some((ids, to)) = account_drop_ins(&p) else {
        return;
    };
    let m = nobody(&p.config);
    crate::service::uninstall::give_kept_to_root(
        &m,
        std::slice::from_ref(&p.config),
        Some(m.folder()),
    );
    assert_eq!(owner(&to.join("apps.yaml")), (0, 0));
    assert_eq!(owner(&to), (0, 0));
    assert_eq!(
        owner(&to.join("locked.yaml")),
        (0, 0),
        "a locked file keeps no group of the removed account"
    );
    assert_eq!(
        owner(&to.join("twice.yaml")),
        ids,
        "a file with two links is not followed"
    );
}

#[cfg(unix)]
#[test]
fn a_reinstall_elsewhere_gives_the_old_drop_ins_to_root_unless_the_new_config_uses_them() {
    let p = place();
    let Some((ids, to)) = account_drop_ins(&p) else {
        return;
    };
    let old = nobody(&p.config);
    let backend = crate::service::tests::Recorder(Default::default(), false);
    let release = |new: &crate::install::Manifest| {
        crate::service::previous::release_previous(&backend, &old, new, &mut Vec::new()).unwrap()
    };
    release(&nobody(&p.config.with_file_name("server.yml")));
    assert_eq!(
        owner(&to.join("apps.yaml")),
        ids,
        "server.yml uses server.d too"
    );
    release(&nobody(&p.config.with_file_name("other/server.yaml")));
    assert_eq!(owner(&to.join("apps.yaml")), (0, 0));
    assert_eq!(owner(&to), (0, 0));
    assert_eq!(owner(&to.join("locked.yaml")), (0, 0));
}

#[cfg(unix)]
#[test]
fn a_reinstall_from_the_installed_path_gives_locked_drop_ins_the_service_group() {
    use std::os::unix::fs::{PermissionsExt, chown};
    let p = place();
    let Some((ids, to)) = account_drop_ins(&p) else {
        return;
    };
    let lock = |path: &Path| {
        chown(path, Some(0), Some(0)).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o440)).unwrap();
    };
    lock(&to.join("locked.yaml"));
    let outside = p.config.with_file_name("outside.yaml");
    write(&outside, "visitors: {}\n");
    lock(&outside);
    std::fs::hard_link(&outside, to.join("linked.yaml")).unwrap();
    write(&to.join("root.yaml"), "services: {}\n");
    let m = nobody(&p.config);
    let backend = crate::service::tests::Recorder(Default::default(), false);
    crate::service::install::place_files(
        &backend,
        None,
        (&std::fs::canonicalize(&p.config).unwrap(), m.folder()),
        (&m, &BTreeMap::new(), &[]),
        |_| Ok(()),
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(owner(&to.join("locked.yaml")), (0, ids.1));
    assert_eq!(owner(&to.join("linked.yaml")), (0, 0), "two links");
    assert_eq!(owner(&to.join("root.yaml")), (0, 0), "not locked");
    assert_eq!(owner(&to.join("apps.yaml")), ids);
}

#[cfg(unix)]
#[test]
fn old_drop_ins_are_shared_when_another_install_reads_the_same_folder() {
    let old = Path::new("/etc/burrow/home/server.yaml");
    let m = |config: &str| crate::install::Manifest {
        config: config.into(),
        ..crate::service::tests::record(crate::install::Platform::Linux)
    };
    let same = [m("/etc/burrow/home/server.yml")];
    assert!(crate::service::previous::shares_drop_ins(old, same.iter()));
    let apart = [
        m("/etc/burrow/away/server.yaml"),
        m("/etc/burrow/home/client.yaml"),
    ];
    assert!(!crate::service::previous::shares_drop_ins(
        old,
        apart.iter()
    ));
}
