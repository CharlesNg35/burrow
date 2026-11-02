use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;
use tracing::warn;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Watching {
    pub(super) files: Vec<PathBuf>,
    pub(super) folders: Vec<PathBuf>,
}

pub(super) fn listing(folder: &Path) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut names: Vec<_> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    names.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for path in names {
        path.file_name().hash(&mut h);
        std::fs::read(&path).ok().hash(&mut h);
    }
    Some(h.finish())
}

pub(super) fn fingerprint(w: &Watching) -> Vec<(PathBuf, Option<u64>)> {
    use std::hash::{Hash, Hasher};
    let folders = w.folders.iter().map(|f| (real(f), listing(f)));
    w.files
        .iter()
        .map(|f| {
            let digest = std::fs::read(f).ok().map(|bytes| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                bytes.hash(&mut h);
                h.finish()
            });
            (real(f), digest)
        })
        .chain(folders)
        .collect()
}

pub(super) fn by_hand() -> &'static str {
    if cfg!(windows) {
        "apply changes with: burrow service reload, or by restarting it"
    } else {
        "apply changes with SIGHUP or: burrow service reload"
    }
}

pub(super) fn dir_of(p: &Path) -> PathBuf {
    p.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

pub(super) fn real(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

pub(super) fn watched_dirs(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for file in files {
        let mut p = file.clone();
        for _ in 0..16 {
            dirs.push(real(&dir_of(&p)));
            for linked in p.ancestors().skip(1) {
                let holder = real(&dir_of(linked));
                if holder.parent().is_some()
                    && std::fs::symlink_metadata(linked).is_ok_and(|m| m.file_type().is_symlink())
                {
                    dirs.push(holder);
                }
            }
            match std::fs::read_link(&p) {
                Ok(t) if t.is_absolute() => p = t,
                Ok(t) => p = dir_of(&p).join(t),
                Err(_) => break,
            }
        }
        dirs.push(dir_of(&real(file)));
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

pub(super) const KQUEUE: bool = cfg!(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
));

pub(super) type Identity = Option<(u64, u64)>;

pub(super) fn identity(path: &Path) -> Identity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[derive(Default)]
pub(super) struct Watch {
    watcher: Option<notify::RecommendedWatcher>,
    tried: Vec<(PathBuf, Identity)>,
    stale: Arc<AtomicBool>,
    failing: Vec<PathBuf>,
    broken: bool,
}

#[cfg(test)]
pub(super) fn renew(
    watch: &mut Watch,
    files: &[PathBuf],
    tx: &mpsc::Sender<&'static str>,
    event: bool,
) {
    let w = Watching {
        files: files.to_vec(),
        folders: Vec::new(),
    };
    renew_set(watch, &w, tx, event);
}

pub(super) fn renew_set(
    watch: &mut Watch,
    w: &Watching,
    tx: &mpsc::Sender<&'static str>,
    event: bool,
) {
    let files = w.files.as_slice();
    let mut dirs = watched_dirs(files);
    dirs.extend(w.folders.iter().filter(|f| f.is_dir()).map(|f| real(f)));
    dirs.sort();
    dirs.dedup();
    let wanted: Vec<(PathBuf, Identity)> = dirs
        .iter()
        .chain(files)
        .map(|p| (p.clone(), identity(p)))
        .collect();
    let stale = watch.stale.load(Ordering::Acquire);
    if wanted == watch.tried && !(KQUEUE && event) && !stale {
        return;
    }
    if std::thread::Builder::new()
        .spawn(|| {})
        .map(std::thread::JoinHandle::join)
        .is_err()
    {
        return;
    }
    let (fresh, failed) = match watch_files(&dirs, files, tx.clone(), watch.stale.clone()) {
        Ok(made) => made,
        Err(e) => {
            if !std::mem::replace(&mut watch.broken, true) {
                warn!("cannot watch the config for changes ({e}); {}", by_hand());
            }
            return;
        }
    };
    watch.broken = false;
    watch.tried = wanted;
    watch.stale.store(false, Ordering::Release);
    for (dir, e) in &failed {
        if !watch.failing.contains(dir) {
            warn!(
                "cannot watch {} for changes ({e}); {}",
                dir.display(),
                by_hand()
            );
        }
    }
    watch.failing = failed.into_iter().map(|(dir, _)| dir).collect();
    if watch.failing.is_empty() || watch.watcher.is_none() {
        watch.watcher = Some(fresh);
    }
}

pub(super) fn watch_files(
    dirs: &[PathBuf],
    files: &[PathBuf],
    tx: mpsc::Sender<&'static str>,
    stale: Arc<AtomicBool>,
) -> Result<(notify::RecommendedWatcher, Vec<(PathBuf, notify::Error)>), notify::Error> {
    use notify::{EventKind, RecursiveMode, Watcher};
    let relevant_dirs = dirs.to_vec();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        if !matches!(
            ev.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return;
        }
        if matches!(ev.kind, EventKind::Remove(_))
            && ev.paths.iter().any(|p| relevant_dirs.contains(p))
        {
            stale.store(true, Ordering::Release);
        }
        let relevant = ev.paths.iter().any(|p| {
            p.parent()
                .map(real)
                .is_some_and(|d| relevant_dirs.contains(&d))
        });
        if relevant {
            let _ = tx.try_send("file changed");
        }
    })?;
    let mut failed = Vec::new();
    for dir in dirs {
        if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
            failed.push((dir.clone(), e));
        }
    }
    for file in files {
        let _ = watcher.watch(file, RecursiveMode::NonRecursive);
    }
    Ok((watcher, failed))
}
