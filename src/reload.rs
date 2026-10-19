use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::settings::{ClientSettings, ServerSettings, VisitorSettings};
use crate::config::{Loaded, Role, load_file};
use crate::diagnostics::Style;
use crate::run::Env;
use crate::validate::startup_report;

pub trait Watched: Clone + PartialEq + Send + 'static {
    const ROLE: Role;
    fn from_loaded(loaded: Loaded) -> Option<Self>;
    fn files(&self, config: &Path) -> Vec<PathBuf> {
        vec![config.to_path_buf()]
    }
}

impl Watched for ServerSettings {
    const ROLE: Role = Role::Server;
    fn from_loaded(loaded: Loaded) -> Option<Self> {
        match loaded {
            Loaded::Server(s, _) => Some(*s),
            _ => None,
        }
    }
    fn files(&self, config: &Path) -> Vec<PathBuf> {
        let mut files = vec![config.to_path_buf()];
        if let Some(tls) = self.websocket.as_ref().and_then(|w| w.tls.as_ref()) {
            files.push(tls.cert_path.clone());
            files.push(tls.key_path.clone());
        }
        files
    }
}

impl Watched for ClientSettings {
    const ROLE: Role = Role::Client;
    fn from_loaded(loaded: Loaded) -> Option<Self> {
        match loaded {
            Loaded::Client(s, _) => Some(*s),
            _ => None,
        }
    }
    fn files(&self, config: &Path) -> Vec<PathBuf> {
        std::iter::once(config.to_path_buf())
            .chain(self.ca_file.clone())
            .collect()
    }
}

impl Watched for VisitorSettings {
    const ROLE: Role = Role::Visitor;
    fn from_loaded(loaded: Loaded) -> Option<Self> {
        match loaded {
            Loaded::Visitor(s, _) => Some(*s),
            _ => None,
        }
    }
    fn files(&self, config: &Path) -> Vec<PathBuf> {
        std::iter::once(config.to_path_buf())
            .chain(self.ca_file.clone())
            .collect()
    }
}

pub type Check<S> = Box<dyn FnMut(S) -> Result<S, String> + Send>;

pub fn accept<S: 'static>() -> Check<S> {
    Box::new(Ok)
}

pub async fn follow<S: Watched>(
    path: PathBuf,
    mut current: S,
    env: Env,
    mut check: Check<S>,
    tx: mpsc::Sender<S>,
) {
    let (trig_tx, mut trig) = mpsc::channel::<&'static str>(16);
    let watch = Watch::default();
    let Some(mut watch) = renewed(watch, current.files(&path), &trig_tx, false).await else {
        return;
    };
    let t = trig_tx.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(RECHECK);
        every.tick().await;
        loop {
            every.tick().await;
            if let Err(mpsc::error::TrySendError::Closed(_)) = t.try_send("file changed") {
                return;
            }
        }
    });
    #[cfg(unix)]
    {
        let t = trig_tx.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let Ok(mut hup) = signal(SignalKind::hangup()) else {
                return;
            };
            while hup.recv().await.is_some() {
                if t.send("SIGHUP").await.is_err() {
                    return;
                }
            }
        });
    }
    let mut rejected = false;
    let mut seen = Vec::new();
    let _ = trig_tx.try_send("file changed");
    while let Some(first) = trig.recv().await {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut why = first;
        while let Ok(more) = trig.try_recv() {
            if more == "SIGHUP" {
                why = more;
            }
        }
        let asked = why == "SIGHUP";
        let files = current.files(&path);
        let triggers = trig_tx.clone();
        let Ok((back, now)) = tokio::task::spawn_blocking(move || {
            let mut watch = watch;
            renew(&mut watch, &files, &triggers, asked);
            let now = fingerprint(&files);
            (watch, now)
        })
        .await
        else {
            return;
        };
        watch = back;
        if now == seen && !asked {
            continue;
        }
        seen = now;
        let (file, vars) = (path.clone(), env.clone());
        let Ok(loaded) =
            tokio::task::spawn_blocking(move || load_file(&file, Some(S::ROLE), &vars)).await
        else {
            return;
        };
        let outcome = match loaded {
            Ok(o) => o,
            Err(e) => {
                let (m, h) = e.describe(&path);
                error!("reload ({why}) rejected: {m} ({h}); keeping the current config");
                rejected = true;
                continue;
            }
        };
        let style = Style::detect(std::io::stderr().is_terminal());
        if outcome.report.has_errors() {
            crate::say_err!("{}", outcome.report.render_text(style));
            error!(
                "reload ({why}) rejected: {} has {} {}; keeping the current config",
                path.display(),
                outcome.report.error_count(),
                crate::diagnostics::plural(outcome.report.error_count(), "error")
            );
            rejected = true;
            continue;
        }
        let Some(next) = outcome.loaded.and_then(S::from_loaded) else {
            continue;
        };
        let next = match check(next) {
            Ok(n) => n,
            Err(m) => {
                error!("reload ({why}) rejected: {m}; keeping the current config");
                rejected = true;
                continue;
            }
        };
        let was_rejected = std::mem::take(&mut rejected);
        if next == current && !asked {
            if was_rejected {
                info!("config is valid again and matches the running config");
            } else {
                debug!("config file changed but settings are the same");
            }
            continue;
        }
        crate::say_err!("{}", startup_report(&outcome.report, style));
        if S::ROLE == Role::Server {
            info!("reloading config ({why})");
        }
        current = next.clone();
        let Some(back) = renewed(watch, current.files(&path), &trig_tx, false).await else {
            return;
        };
        watch = back;
        if tx.send(next).await.is_err() {
            return;
        }
    }
}

fn fingerprint(files: &[PathBuf]) -> Vec<(PathBuf, Option<u64>)> {
    use std::hash::{Hash, Hasher};
    files
        .iter()
        .map(|f| {
            let digest = std::fs::read(f).ok().map(|bytes| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                bytes.hash(&mut h);
                h.finish()
            });
            (real(f), digest)
        })
        .collect()
}

fn by_hand() -> &'static str {
    if cfg!(windows) {
        "apply changes with: burrow service reload, or by restarting it"
    } else {
        "apply changes with SIGHUP or: burrow service reload"
    }
}

fn dir_of(p: &Path) -> PathBuf {
    p.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn real(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn watched_dirs(files: &[PathBuf]) -> Vec<PathBuf> {
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

const KQUEUE: bool = cfg!(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
));

const RECHECK: Duration = Duration::from_secs(30);

type Identity = Option<(u64, u64)>;

fn identity(path: &Path) -> Identity {
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
struct Watch {
    watcher: Option<notify::RecommendedWatcher>,
    watched: Vec<(PathBuf, Identity)>,
    stale: Arc<AtomicBool>,
    failing: Vec<PathBuf>,
    broken: bool,
}

async fn renewed(
    mut watch: Watch,
    files: Vec<PathBuf>,
    tx: &mpsc::Sender<&'static str>,
    force: bool,
) -> Option<Watch> {
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        renew(&mut watch, &files, &tx, force);
        watch
    })
    .await
    .ok()
}

fn renew(watch: &mut Watch, files: &[PathBuf], tx: &mpsc::Sender<&'static str>, force: bool) {
    let dirs = watched_dirs(files);
    let wanted: Vec<(PathBuf, Identity)> = dirs
        .iter()
        .chain(files)
        .map(|p| (p.clone(), identity(p)))
        .collect();
    let stale = watch.stale.load(Ordering::Acquire);
    if wanted == watch.watched && !KQUEUE && !force && !stale {
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
    if watch.failing.is_empty() {
        watch.watcher = Some(fresh);
        watch.watched = wanted;
        watch.stale.store(false, Ordering::Release);
    } else if watch.watcher.is_none() {
        watch.watcher = Some(fresh);
    }
}

fn watch_files(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn a_config_folder_replaced_at_the_same_path_is_watched_again() {
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("cfg");
        let config = folder.join("server.yaml");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(&config, "a").unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        let mut watch = Watch::default();
        renew(&mut watch, std::slice::from_ref(&config), &tx, false);
        std::fs::remove_dir_all(&folder).unwrap();
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(&config, "b").unwrap();
        let told = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(matches!(told, Ok(Some(_))), "the removal was not reported");
        tokio::time::sleep(Duration::from_millis(200)).await;
        renew(&mut watch, std::slice::from_ref(&config), &tx, false);
        tokio::time::sleep(Duration::from_millis(300)).await;
        while rx.try_recv().is_ok() {}
        std::fs::write(&config, "c").unwrap();
        let seen = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(matches!(seen, Ok(Some(_))), "the replaced folder is not watched");
    }

    #[tokio::test]
    async fn a_failed_renewal_keeps_the_watcher_that_works() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("server.yaml");
        std::fs::write(&config, "a").unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        let mut watch = Watch::default();
        renew(&mut watch, std::slice::from_ref(&config), &tx, false);
        let kept = watch.watched.clone();
        let gone = dir.path().join("missing").join("server.yaml");
        renew(&mut watch, &[gone], &tx, false);
        assert_eq!(watch.watched, kept);
        tokio::time::sleep(Duration::from_millis(300)).await;
        while rx.try_recv().is_ok() {}
        std::fs::write(&config, "b").unwrap();
        let seen = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(matches!(seen, Ok(Some(_))), "the working watcher was dropped");
    }

    #[test]
    fn links_at_the_top_of_the_disk_never_make_the_whole_disk_watched() {
        let files = [
            PathBuf::from("/etc/burrow/home.yaml"),
            PathBuf::from("/tmp/burrow/home.yaml"),
            PathBuf::from("/var/lib/burrow/home.yaml"),
        ];
        let dirs = watched_dirs(&files);
        assert!(dirs.iter().all(|d| d.parent().is_some()), "{dirs:?}");
    }
}
