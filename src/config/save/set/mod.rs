use std::path::{Path, PathBuf};

use super::platform::{self, Hidden, Pending};
use super::{Refused, write_back};
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
    Hide(Hidden),
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
        Change::Remove { before } => Ok(Step::Hide(platform::stage_remove(path, before)?)),
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
        Step::Hide(h) => h.commit(),
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
        (Change::Remove { .. }, Step::Hide(h)) => h.undo(),
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
            let refused = undo_all(&staged[..at], e);
            drop_made(&staged);
            return Err(refused);
        }
    }
    for s in &staged {
        if let Step::Hide(h) = &s.step {
            h.finish();
        }
    }
    Ok(())
}

fn drop_made(staged: &[Staged]) {
    for s in staged {
        if let Step::Write(p) = &s.step
            && let Some(d) = p.made()
        {
            let _ = std::fs::remove_dir(d);
        }
    }
}

fn discard(staged: &[Staged]) {
    for s in staged {
        if let Step::Write(p) = &s.step {
            p.discard();
        }
    }
    drop_made(staged);
}

#[cfg(test)]
mod tests;
