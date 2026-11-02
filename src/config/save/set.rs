use std::path::{Path, PathBuf};

use super::platform::{self, Pending};
use super::{Refused, spare_prefix, write_back};
use crate::config::docs::{Docs, read_only};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Replace { before: String, after: String },
    Create { after: String },
    Remove { before: String },
}

fn items(text: &str) -> std::collections::BTreeSet<(String, String)> {
    let (_, _, src) = crate::config::patch::split_text(text);
    let mut out = std::collections::BTreeSet::new();
    if let Ok(root) = crate::config::tree::parse(&src)
        && let crate::config::tree::Value::Map(top) = root.value
    {
        for section in top
            .into_iter()
            .filter(|e| burrow_config::drop_ins::is_section(&e.key))
        {
            if let crate::config::tree::Value::Map(list) = section.value.value {
                out.extend(list.into_iter().map(|i| (section.key.clone(), i.key)));
            }
        }
    }
    out
}

fn loses(before: &str, after: &str) -> bool {
    !items(before).is_subset(&items(after))
}

pub fn changes(before: &Docs, after: &Docs) -> Vec<(String, Change)> {
    let mut out = Vec::new();
    for d in &after.list {
        match before.text_of(&d.name) {
            Some(old) if old == d.text => {}
            Some(old) => out.push((
                d.name.clone(),
                Change::Replace {
                    before: old.to_string(),
                    after: d.text.clone(),
                },
            )),
            None => out.push((
                d.name.clone(),
                Change::Create {
                    after: d.text.clone(),
                },
            )),
        }
    }
    for d in &before.list {
        if after.get(&d.name).is_none() {
            out.push((
                d.name.clone(),
                Change::Remove {
                    before: d.text.clone(),
                },
            ));
        }
    }
    out.sort_by_key(|(_, c)| match c {
        Change::Replace { before, after } if loses(before, after) => 0,
        Change::Remove { before } if loses(before, "") => 0,
        Change::Replace { .. } => 1,
        Change::Create { .. } => 2,
        Change::Remove { .. } => 3,
    });
    out
}

enum Step {
    Write(Pending),
    Hide { real: PathBuf, spare: PathBuf },
}

struct Staged {
    path: PathBuf,
    change: Change,
    step: Step,
}

fn stage(main: &Path, path: &Path, change: &Change) -> Result<Step, Refused> {
    match change {
        Change::Replace { before, after } => {
            Ok(Step::Write(platform::stage_replace(path, before, after)?))
        }
        Change::Create { after } => Ok(Step::Write(platform::stage_create(path, after, main)?)),
        Change::Remove { before } => {
            let real = std::fs::canonicalize(path)?;
            if std::fs::read_to_string(&real)? != *before {
                return Err(Refused::Changed);
            }
            let name = real.file_name().map(|n| n.to_string_lossy().into_owned());
            let spare = real.with_file_name(format!(
                "{}{}",
                spare_prefix(&name.unwrap_or_default()),
                std::process::id()
            ));
            Ok(Step::Hide { real, spare })
        }
    }
}

#[cfg(test)]
thread_local! {
    pub static FAIL_AT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    pub static FAIL_UNDO: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn commit(step: &Step, _at: usize) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL_AT.with(|f| f.get()) == Some(_at) {
        if let Step::Write(p) = step {
            p.discard();
        }
        return Err(std::io::Error::other("injected failure"));
    }
    match step {
        Step::Write(p) => p.commit(),
        Step::Hide { real, spare } => std::fs::rename(real, spare),
    }
}

fn undo(s: &Staged) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL_UNDO.with(|f| f.get()) {
        return Err(std::io::Error::other("injected undo failure"));
    }
    match (&s.change, &s.step) {
        (Change::Replace { before, after }, _) => match write_back(&s.path, after, before) {
            Ok(()) => Ok(()),
            Err(Refused::Changed) => Err(std::io::Error::other("it changed meanwhile")),
            Err(Refused::Io(e)) => Err(e),
        },
        (Change::Create { .. }, _) => std::fs::remove_file(&s.path),
        (Change::Remove { .. }, Step::Hide { real, spare }) => std::fs::rename(spare, real),
        (Change::Remove { .. }, Step::Write(_)) => Ok(()),
    }
}

fn undo_all(done: &[Staged], failed: std::io::Error) -> Refused {
    let mut lost = Vec::new();
    for s in done.iter().rev() {
        if let Err(e) = undo(s) {
            tracing::error!(
                "could not put back {} after a failed save: {e}; the earlier version is kept in the history",
                s.path.display()
            );
            lost.push(s.path.display().to_string());
        }
    }
    if lost.is_empty() {
        return Refused::Io(failed);
    }
    Refused::Io(std::io::Error::other(format!(
        "could not restore {} files: {} (after: {failed}); the earlier version is kept in the history",
        lost.len(),
        lost.join(", ")
    )))
}

pub fn write_set(main: &Path, before: &Docs, after: &Docs) -> Result<(), Refused> {
    let mut staged: Vec<Staged> = Vec::new();
    for (name, change) in changes(before, after) {
        let Some(path) = crate::config::docs::path_of(main, &name) else {
            discard(&staged);
            return Err(Refused::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{name} is not a file in the config's drop-in folder"),
            )));
        };
        let locked = (name != before.main().name && !matches!(change, Change::Create { .. }))
            .then(|| read_only(&path))
            .flatten();
        let step = if let Some(why) = locked {
            Err(Refused::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{name} is read-only on disk: {}", why.why()),
            )))
        } else {
            stage(main, &path, &change)
        };
        match step {
            Ok(step) => staged.push(Staged { path, change, step }),
            Err(e) => {
                discard(&staged);
                return Err(e);
            }
        }
    }
    for (at, s) in staged.iter().enumerate() {
        if let Err(e) = commit(&s.step, at) {
            discard(&staged[at + 1..]);
            return Err(undo_all(&staged[..at], e));
        }
    }
    for s in &staged {
        if let Step::Hide { spare, .. } = &s.step {
            let _ = std::fs::remove_file(spare);
        }
    }
    Ok(())
}

fn discard(staged: &[Staged]) {
    for s in staged {
        if let Step::Write(p) = &s.step {
            p.discard();
        }
    }
}

#[cfg(test)]
mod tests {
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
        let order: Vec<String> = changes(&before, &split).into_iter().map(|(n, _)| n).collect();
        assert_eq!(order, ["server.yaml", "server.d/web.yaml"]);
        let merge = docs(&[
            ("server.yaml", "services:\n  web: {}\n  db: {}\n  blog: {}\n"),
            ("server.d/a.yaml", "services: {}\n"),
            ("server.d/b.yaml", "clients:\n  home: {}\n"),
        ]);
        let order: Vec<String> = changes(&before, &merge).into_iter().map(|(n, _)| n).collect();
        assert_eq!(order, ["server.d/a.yaml", "server.yaml"]);
        let revert = docs(&[("server.yaml", "services:\n  web: {}\n  db: {}\n  blog: {}\n  x: {}\n")]);
        let order: Vec<String> = changes(&before, &revert).into_iter().map(|(n, _)| n).collect();
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
}
