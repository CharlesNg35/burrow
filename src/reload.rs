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
    let mut watched = current.files(&path);
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
    while let Some(first) = trig.recv().await {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut why = first;
        while let Ok(more) = trig.try_recv() {
            if more == "SIGHUP" {
                why = more;
            }
        }
        let asked = why == "SIGHUP";
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
        let files = current.files(&path);
        if files != watched {
            drop(watcher.take());
            watcher = watch_files(&files, trig_tx.clone());
            watched = files;
        }
        if tx.send(next).await.is_err() {
            return;
        }
    }
}

fn watch_files(
    files: &[PathBuf],
    tx: mpsc::Sender<&'static str>,
) -> Option<notify::RecommendedWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};
    let dir_of = |p: &Path| {
        p.parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    };
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (config, certs) = files.split_first()?;
    let config_dir = real(&dir_of(config));
    let config_name = config.file_name()?.to_os_string();
    let mut cert_dirs: Vec<PathBuf> = certs
        .iter()
        .flat_map(|f| [real(&dir_of(f)), dir_of(&real(f))])
        .collect();
    cert_dirs.sort();
    cert_dirs.dedup();
    let mut dirs = cert_dirs.clone();
    dirs.push(config_dir.clone());
    dirs.sort();
    dirs.dedup();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        if !matches!(
            ev.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return;
        }
        let relevant = ev.paths.iter().any(|p| {
            let dir = p.parent().map(real);
            dir.as_ref().is_some_and(|d| cert_dirs.contains(d))
                || (dir.as_ref() == Some(&config_dir)
                    && p.file_name() == Some(config_name.as_os_str()))
        });
        if relevant {
            let _ = tx.try_send("file changed");
        }
    })
    .map_err(|e| warn!("cannot watch the config for changes ({e}); reload with SIGHUP instead"))
    .ok()?;
    for dir in dirs {
        if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
            warn!(
                "cannot watch {} for changes ({e}); reload with SIGHUP instead",
                dir.display()
            );
        }
    }
    Some(watcher)
}
