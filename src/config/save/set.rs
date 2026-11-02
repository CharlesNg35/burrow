use std::path::{Path, PathBuf};

use super::platform::{self, Pending};
use super::{Refused, spare_prefix, write_back};
use crate::config::docs::{Docs, folder_of, read_only};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Replace { before: String, after: String },
    Create { after: String },
    Remove { before: String },
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
    let main = before.main().name.clone();
    out.sort_by_key(|(name, c)| match c {
        Change::Create { .. } => 0,
        Change::Replace { .. } if *name != main => 1,
        Change::Replace { .. } => 2,
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

fn path_of(main: &Path, before: &Docs, name: &str) -> PathBuf {
    if name == before.main().name {
        main.to_path_buf()
    } else {
        folder_of(main).join(name)
    }
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

fn undo(s: &Staged) {
    match (&s.change, &s.step) {
        (Change::Replace { before, after }, _) => {
            let _ = write_back(&s.path, after, before);
        }
        (Change::Create { .. }, _) => {
            let _ = std::fs::remove_file(&s.path);
        }
        (Change::Remove { .. }, Step::Hide { real, spare }) => {
            let _ = std::fs::rename(spare, real);
        }
        (Change::Remove { .. }, Step::Write(_)) => {}
    }
}

pub fn write_set(main: &Path, before: &Docs, after: &Docs) -> Result<(), Refused> {
    let mut staged: Vec<Staged> = Vec::new();
    for (name, change) in changes(before, after) {
        let path = path_of(main, before, &name);
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
            staged[..at].iter().rev().for_each(undo);
            discard(&staged[at + 1..]);
            return Err(Refused::Io(e));
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
