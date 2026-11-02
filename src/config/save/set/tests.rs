#![cfg(test)]
use super::*;
use crate::config::docs::Doc;

fn docs(list: &[(&str, &str)]) -> Docs {
    Docs {
        list: list
            .iter()
            .map(|(n, t)| Doc {
                name: n.to_string(),
                text: t.to_string(),
            })
            .collect(),
    }
}

fn read(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok()
}

fn listing(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = walk(dir, dir);
    out.sort();
    out
}

fn walk(root: &Path, dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(root, &p));
        } else {
            out.push(p.strip_prefix(root).unwrap().display().to_string());
        }
    }
    out
}

fn setup() -> (tempfile::TempDir, PathBuf, Docs) {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("server.yaml");
    std::fs::write(&main, "listen: 1\n").unwrap();
    std::fs::create_dir(dir.path().join("server.d")).unwrap();
    std::fs::write(dir.path().join("server.d/a.yaml"), "services: {}\n").unwrap();
    std::fs::write(dir.path().join("server.d/gone.yaml"), "clients: {}\n").unwrap();
    let before = docs(&[
        ("server.yaml", "listen: 1\n"),
        ("server.d/a.yaml", "services: {}\n"),
        ("server.d/gone.yaml", "clients: {}\n"),
    ]);
    (dir, main, before)
}

#[test]
fn a_file_that_loses_an_item_is_written_before_the_file_that_gains_it() {
    let before = docs(&[
        ("server.yaml", "services:\n  web: {}\n  db: {}\n"),
        ("server.d/a.yaml", "services:\n  blog: {}\n"),
        ("server.d/b.yaml", "clients:\n  home: {}\n"),
    ]);
    let split = docs(&[
        ("server.yaml", "services:\n  db: {}\n"),
        ("server.d/a.yaml", "services:\n  blog: {}\n"),
        ("server.d/b.yaml", "clients:\n  home: {}\n"),
        ("server.d/web.yaml", "services:\n  web: {}\n"),
    ]);
    let order: Vec<String> = changes(&before, &split)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(order, ["server.yaml", "server.d/web.yaml"]);
    let merge = docs(&[
        (
            "server.yaml",
            "services:\n  web: {}\n  db: {}\n  blog: {}\n",
        ),
        ("server.d/a.yaml", "services: {}\n"),
        ("server.d/b.yaml", "clients:\n  home: {}\n"),
    ]);
    let order: Vec<String> = changes(&before, &merge)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(order, ["server.d/a.yaml", "server.yaml"]);
    let revert = docs(&[(
        "server.yaml",
        "services:\n  web: {}\n  db: {}\n  blog: {}\n  x: {}\n",
    )]);
    let order: Vec<String> = changes(&before, &revert)
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(order, ["server.d/a.yaml", "server.d/b.yaml", "server.yaml"]);
}

#[test]
fn a_change_over_several_files_writes_creates_and_removes_them_together() {
    let (dir, main, before) = setup();
    let after = docs(&[
        ("server.yaml", "listen: 2\n"),
        ("server.d/a.yaml", "services: { x: {} }\n"),
        ("server.d/new.yaml", "visitors: {}\n"),
    ]);
    write_set(&main, &before, &after).unwrap();
    assert_eq!(read(dir.path(), "server.yaml").unwrap(), "listen: 2\n");
    assert_eq!(
        read(dir.path(), "server.d/a.yaml").unwrap(),
        "services: { x: {} }\n"
    );
    assert_eq!(
        read(dir.path(), "server.d/new.yaml").unwrap(),
        "visitors: {}\n"
    );
    assert_eq!(
        listing(dir.path()),
        ["server.d/a.yaml", "server.d/new.yaml", "server.yaml"]
            .map(|s| s.replace('/', std::path::MAIN_SEPARATOR_STR))
    );
}

#[test]
fn a_failure_between_renames_puts_every_file_back() {
    for at in 0..4 {
        let (dir, main, before) = setup();
        let after = docs(&[
            ("server.yaml", "listen: 2\n"),
            ("server.d/a.yaml", "services: { x: {} }\n"),
            ("server.d/new.yaml", "visitors: {}\n"),
        ]);
        FAIL_AT.with(|f| f.set(Some(at)));
        let r = write_set(&main, &before, &after);
        FAIL_AT.with(|f| f.set(None));
        assert!(r.is_err(), "failure at step {at} is reported");
        assert_eq!(read(dir.path(), "server.yaml").unwrap(), "listen: 1\n");
        assert_eq!(
            read(dir.path(), "server.d/a.yaml").unwrap(),
            "services: {}\n"
        );
        assert_eq!(
            read(dir.path(), "server.d/gone.yaml").unwrap(),
            "clients: {}\n"
        );
        assert_eq!(
            listing(dir.path()),
            ["server.d/a.yaml", "server.d/gone.yaml", "server.yaml"]
                .map(|s| s.replace('/', std::path::MAIN_SEPARATOR_STR)),
            "step {at}: no file is added or left behind"
        );
    }
}

#[test]
fn a_failed_undo_is_reported_with_the_files_it_could_not_put_back() {
    let (_dir, main, before) = setup();
    let after = docs(&[
        ("server.yaml", "listen: 2\n"),
        ("server.d/a.yaml", "services: { x: {} }\n"),
        ("server.d/new.yaml", "visitors: {}\n"),
    ]);
    FAIL_AT.with(|f| f.set(Some(2)));
    FAIL_UNDO.with(|f| f.set(true));
    let r = write_set(&main, &before, &after);
    FAIL_AT.with(|f| f.set(None));
    FAIL_UNDO.with(|f| f.set(false));
    let Err(Refused::Io(e)) = r else {
        panic!("{r:?}")
    };
    let text = e.to_string();
    assert!(text.starts_with("could not restore 2 files: "), "{text}");
    assert!(
        text.contains("a.yaml") && text.contains("server.yaml"),
        "{text}"
    );
}

#[test]
fn a_file_changed_on_disk_stops_the_whole_change_before_anything_is_written() {
    let (dir, main, before) = setup();
    std::fs::write(dir.path().join("server.d/a.yaml"), "services: { y: {} }\n").unwrap();
    let after = docs(&[
        ("server.yaml", "listen: 2\n"),
        ("server.d/a.yaml", "services: { x: {} }\n"),
        ("server.d/gone.yaml", "clients: {}\n"),
    ]);
    assert!(matches!(
        write_set(&main, &before, &after),
        Err(Refused::Changed)
    ));
    assert_eq!(read(dir.path(), "server.yaml").unwrap(), "listen: 1\n");
    assert_eq!(listing(dir.path()).len(), 3, "no temporary file is left");
}

#[test]
fn a_drop_in_marked_read_only_is_never_replaced_even_when_the_folder_allows_it() {
    let (dir, main, before) = setup();
    let locked = dir.path().join("server.d/a.yaml");
    let mut perms = std::fs::metadata(&locked).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&locked, perms).unwrap();
    let mut after = before.clone();
    after.list[0].text = "listen: 2\n".into();
    after.list[1].text = "services: { x: {} }\n".into();
    let r = write_set(&main, &before, &after);
    assert!(
        matches!(r, Err(Refused::Io(ref e)) if e.kind() == std::io::ErrorKind::PermissionDenied),
        "{r:?}"
    );
    assert_eq!(read(dir.path(), "server.yaml").unwrap(), "listen: 1\n");
    assert_eq!(
        read(dir.path(), "server.d/a.yaml").unwrap(),
        "services: {}\n"
    );
    let mut gone = before.clone();
    gone.list.remove(1);
    assert!(write_set(&main, &before, &gone).is_err());
    assert!(locked.exists());
    #[cfg(windows)]
    {
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&locked, perms).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn a_drop_in_that_links_elsewhere_is_read_but_never_written() {
    let (dir, main, before) = setup();
    let outside = dir.path().join("outside.yaml");
    std::fs::write(&outside, "services: {}\n").unwrap();
    let link = dir.path().join("server.d/a.yaml");
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert_eq!(
        crate::config::docs::read(&main)
            .unwrap()
            .text_of("server.d/a.yaml"),
        Some("services: {}\n")
    );
    let mut after = before.clone();
    after.list[1].text = "services: { x: {} }\n".into();
    assert!(write_set(&main, &before, &after).is_err());
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "services: {}\n");
}

#[test]
fn a_name_that_leaves_the_folder_is_never_written() {
    let (dir, main, before) = setup();
    for bad in ["server.d/../x.yaml", "../x.yaml", "/tmp/burrow-escape.yaml"] {
        let mut after = before.clone();
        after.list.push(Doc {
            name: bad.into(),
            text: "services: {}\n".into(),
        });
        assert!(write_set(&main, &before, &after).is_err(), "{bad}");
    }
    assert!(!dir.path().join("x.yaml").exists());
    assert!(!dir.path().parent().unwrap().join("x.yaml").exists());
    assert!(!std::path::Path::new("/tmp/burrow-escape.yaml").exists());
}

#[test]
fn the_folder_made_for_a_new_file_is_private_and_gone_again_when_the_save_fails() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("server.yaml");
    std::fs::write(&main, "listen: 1\n").unwrap();
    let before = docs(&[("server.yaml", "listen: 1\n")]);
    let after = docs(&[
        ("server.yaml", "listen: 2\n"),
        ("server.d/new.yaml", "visitors: {}\n"),
    ]);
    for at in 0..2 {
        FAIL_AT.with(|f| f.set(Some(at)));
        let r = write_set(&main, &before, &after);
        FAIL_AT.with(|f| f.set(None));
        assert!(r.is_err());
        assert!(
            !dir.path().join("server.d").exists(),
            "step {at}: no folder is left"
        );
    }
    write_set(&main, &before, &after).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let folder = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o750;
        let made = std::fs::metadata(dir.path().join("server.d"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(made & 0o777, folder);
    }
}

#[test]
fn a_new_file_never_replaces_one_that_appeared_meanwhile() {
    let (dir, main, before) = setup();
    std::fs::write(dir.path().join("server.d/new.yaml"), "mine\n").unwrap();
    let mut after = before.clone();
    after.list.push(Doc {
        name: "server.d/new.yaml".into(),
        text: "visitors: {}\n".into(),
    });
    assert!(matches!(
        write_set(&main, &before, &after),
        Err(Refused::Changed)
    ));
    assert_eq!(read(dir.path(), "server.d/new.yaml").unwrap(), "mine\n");
}

#[cfg(unix)]
#[test]
fn a_folder_sync_that_fails_after_the_rename_does_not_undo_the_save() {
    let (dir, main, before) = setup();
    let after = docs(&[
        ("server.yaml", "listen: 2\n"),
        ("server.d/a.yaml", "services: { x: {} }\n"),
        ("server.d/gone.yaml", "clients: {}\n"),
    ]);
    super::super::unix::FAIL_FSYNC.with(|f| f.set(true));
    let r = write_set(&main, &before, &after);
    super::super::unix::FAIL_FSYNC.with(|f| f.set(false));
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(read(dir.path(), "server.yaml").unwrap(), "listen: 2\n");
    assert_eq!(
        read(dir.path(), "server.d/a.yaml").unwrap(),
        "services: { x: {} }\n"
    );
}

#[test]
fn a_new_file_never_replaces_one_that_appeared_after_staging() {
    let (dir, main, _) = setup();
    let path = dir.path().join("server.d/late.yaml");
    let staged = super::super::platform::stage_create(&path, "visitors: {}\n", &main).unwrap();
    std::fs::write(&path, "written by someone else\n").unwrap();
    assert!(staged.commit().is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "written by someone else\n"
    );
    staged.discard();
    assert_eq!(listing(dir.path()).len(), 4, "no temporary file is left");
}
