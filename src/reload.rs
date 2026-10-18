use std::io::IsTerminal;
use std::path::{Path, PathBuf};
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
    let mut watched = watched_dirs(&current.files(&path));
    let mut watcher = watch_files(&watched, trig_tx.clone());
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
        let dirs = watched_dirs(&current.files(&path));
        if dirs != watched {
            drop(watcher.take());
            watcher = watch_files(&dirs, trig_tx.clone());
            watched = dirs;
        }
        let mut why = first;
        while let Ok(more) = trig.try_recv() {
            if more == "SIGHUP" {
                why = more;
            }
        }
        let asked = why == "SIGHUP";
        let now = fingerprint(&current.files(&path));
        if now == seen && !asked {
            continue;
        }
        seen = now;
        let outcome = match load_file(&path, Some(S::ROLE), &env) {
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
            eprint!("{}", outcome.report.render_text(style));
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
        eprint!("{}", startup_report(&outcome.report, style));
        if S::ROLE == Role::Server {
            info!("reloading config ({why})");
        }
        current = next.clone();
        let dirs = watched_dirs(&current.files(&path));
        if dirs != watched {
            drop(watcher.take());
            watcher = watch_files(&dirs, trig_tx.clone());
            watched = dirs;
        }
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

fn watch_files(
    dirs: &[PathBuf],
    tx: mpsc::Sender<&'static str>,
) -> Option<notify::RecommendedWatcher> {
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
        let relevant = ev.paths.iter().any(|p| {
            p.parent()
                .map(real)
                .is_some_and(|d| relevant_dirs.contains(&d))
        });
        if relevant {
            let _ = tx.try_send("file changed");
        }
    })
    .map_err(|e| warn!("cannot watch the config for changes ({e}); {}", by_hand()))
    .ok()?;
    for dir in dirs {
        if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
            warn!(
                "cannot watch {} for changes ({e}); {}",
                dir.display(),
                by_hand()
            );
        }
    }
    Some(watcher)
}
