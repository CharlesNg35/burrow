use std::fs::{OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use super::note::{
    NOTE_PATIENCE, extra_links, names, no_follow, open_shared, replace_linked, single_file,
    trusted_note, write_note,
};
use super::safe_dir;
use super::{
    Identity, InstanceLock, default_dir, dirs_with, keep_fresh, open_failure, prepare_dir,
};
use crate::base::failure::Failure;

pub fn acquire(identity: &Identity, config: &Path) -> Result<InstanceLock, Failure> {
    let dir = default_dir();
    let elsewhere = dirs_with(&dir)
        .iter()
        .skip(1)
        .find_map(|other| held_elsewhere(other, identity));
    let taken = match elsewhere {
        Some(f) => Err(f),
        None => acquire_in(&dir, identity, config),
    };
    #[cfg(windows)]
    if let Err(f) = &taken
        && AS_SERVICE.load(std::sync::atomic::Ordering::Relaxed)
    {
        crate::say_errln!("warning: {}; a service starts anyway", f.message);
        return Ok(InstanceLock {
            _file: None,
            path: dir.join(format!("{}.lock", identity.key())),
            noted: false,
            _fresh: None,
        });
    }
    taken
}

#[cfg(windows)]
static AS_SERVICE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
pub fn running_as_service() {
    AS_SERVICE.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn acquire_in(dir: &Path, identity: &Identity, config: &Path) -> Result<InstanceLock, Failure> {
    let path = dir.join(format!("{}.lock", identity.key()));
    let prepared = prepare_dir(dir);
    if prepared.is_ok() && !safe_dir(dir) {
        return Ok(unlocked(dir, &path));
    }
    if let Err(e) = prepared {
        if planted(dir) {
            return Ok(unlocked(dir, &path));
        }
        return Err(Failure::new(format!(
            "cannot create {}: {}",
            dir.display(),
            crate::base::failure::file_reason(&e)
        ))
        .hint("burrow keeps a small lock file there so the same tunnel cannot run twice"));
    }
    let mut attempts = 0;
    let (file, writable) = loop {
        attempts += 1;
        let opened = match open_shared(&path) {
            Err(_) if extra_links(&path) => match replace_linked(&path) {
                Ok(Some(opened)) => Ok(opened),
                Ok(None) => return Err(already_running(identity, &path)),
                Err(e) => Err(e),
            },
            other => other,
        };
        let (file, writable) = match opened {
            Ok(opened) => opened,
            Err(_) if planted(&path) => return Ok(unlocked(&path, &path)),
            Err(_) if planted(dir) => return Ok(unlocked(dir, &path)),
            Err(e) => return Err(open_failure(&path, &e)),
        };
        match file.try_lock() {
            Ok(()) if attempts < 5 && !names(&path, &file) => continue,
            Ok(()) => break (file, writable),
            Err(TryLockError::WouldBlock) if !held_by_a_trusted_instance(&path) => {
                crate::say_errln!(
                    "warning: {} is locked by a process burrow does not trust (another user's, or one that left no note); starting anyway",
                    path.display()
                );
                return Ok(InstanceLock {
                    _file: Some(file),
                    path,
                    noted: false,
                    _fresh: None,
                });
            }
            Err(TryLockError::WouldBlock) => return Err(already_running(identity, &path)),
            Err(TryLockError::Error(e)) => {
                return Err(Failure::new(format!(
                    "cannot lock {}: {}",
                    path.display(),
                    crate::base::failure::file_reason(&e)
                )));
            }
        }
    };
    let note = format!(
        "pid {}, config {}",
        std::process::id(),
        crate::base::fs::canonical_or_same(config).display()
    );
    let noted = writable && write_note(&path.with_extension("info"), &note);
    let mut fresh = vec![path.clone()];
    if noted {
        fresh.push(path.with_extension("info"));
    }
    Ok(InstanceLock {
        _file: Some(file),
        path,
        noted,
        _fresh: Some(keep_fresh(fresh)),
    })
}

fn unlocked(entry: &Path, path: &Path) -> InstanceLock {
    crate::say_errln!(
        "warning: {} belongs to another user and cannot be used as a lock; starting without it",
        entry.display()
    );
    InstanceLock {
        _file: None,
        path: path.to_path_buf(),
        noted: false,
        _fresh: None,
    }
}

#[cfg(unix)]
pub(super) fn planted(entry: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let me = rustix::process::geteuid().as_raw();
    std::fs::symlink_metadata(entry).is_ok_and(|m| m.uid() != 0 && m.uid() != me)
}

#[cfg(not(unix))]
pub(super) fn planted(_entry: &Path) -> bool {
    false
}

fn already_running(identity: &Identity, lock: &Path) -> Failure {
    let holder = std::fs::File::open(lock.with_extension("info"))
        .ok()
        .and_then(|f| {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::Read::take(f, 4096), &mut text).ok()?;
            Some(text)
        })
        .unwrap_or_default();
    let mut holder = holder.trim().to_string();
    if let Identity::Server { config } = identity {
        holder = holder.replace(&format!(", config {}", config.display()), "");
    }
    let who = if holder.is_empty() {
        String::new()
    } else {
        format!(" ({holder})")
    };
    let hint = if crate::base::failure::SERVICES {
        format!(
            "stop the other instance first; if it runs as a service, find it with: {}",
            crate::base::failure::cmd("service list")
        )
    } else {
        "stop the other instance first".to_string()
    };
    Failure::new(format!("{} is already running{who}", identity.describe())).hint(hint)
}

pub fn held(identity: &Identity) -> Option<Failure> {
    held_in(&dirs_with(&default_dir()), identity)
}

pub fn held_in(dirs: &[PathBuf], identity: &Identity) -> Option<Failure> {
    dirs.iter().find_map(|dir| held_elsewhere(dir, identity))
}

pub(super) fn held_elsewhere(dir: &Path, identity: &Identity) -> Option<Failure> {
    let path = dir.join(format!("{}.lock", identity.key()));
    let file = no_follow(OpenOptions::new().read(true))
        .open(&path)
        .and_then(single_file)
        .ok()?;
    let busy = matches!(file.try_lock(), Err(TryLockError::WouldBlock));
    (busy && held_by_a_trusted_instance(&path)).then(|| already_running(identity, &path))
}

pub(super) fn held_by_a_trusted_instance(lock: &Path) -> bool {
    let deadline = std::time::Instant::now() + NOTE_PATIENCE;
    loop {
        if trusted_note(&lock.with_extension("info")).is_some() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}
