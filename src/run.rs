use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use crate::client::{Client, RunEnd};
use crate::config::env::ProcessEnv;
use crate::config::settings::ServerSettings;
use crate::config::{Loaded, Role, load_file};
use crate::diagnostics::Style;
use crate::failure::Failure;
use crate::server::Server;
use crate::validate::startup_report;

#[derive(Debug, Clone, Copy)]
pub struct Logging {
    pub level: tracing::Level,
    pub json: bool,
}

pub fn init_logging(l: Logging) {
    let filter = tracing_subscriber::EnvFilter::try_from_env("BURROW_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(format!("burrow={}", l.level)));
    let color = Style::detect(std::io::stderr().is_terminal()).color;
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false);
    if l.json {
        builder.json().flatten_event(true).init();
    } else {
        builder.with_ansi(color).init();
    }
}

fn load(path: &Path, role: Role) -> Result<Loaded, Failure> {
    let outcome = load_file(path, Some(role), &ProcessEnv).map_err(|e| {
        let (m, h) = e.describe(path);
        Failure::new(m).hint(h)
    })?;
    let style = Style::detect(std::io::stderr().is_terminal());
    if outcome.report.has_errors() {
        eprint!("{}", outcome.report.render_text(style));
        return Err(Failure::new(format!(
            "{} has {} {}, not starting",
            path.display(),
            outcome.report.error_count(),
            crate::diagnostics::plural(outcome.report.error_count(), "error")
        ))
        .hint(format!(
            "fix them, then check again with: burrow validate -c {}",
            path.display()
        )));
    }
    eprint!("{}", startup_report(&outcome.report, style));
    outcome
        .loaded
        .ok_or_else(|| Failure::new(format!("{} could not be loaded", path.display())))
}

fn runtime() -> Result<tokio::runtime::Runtime, Failure> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Failure::new(format!("cannot start the async runtime: {e}")))
}

#[cfg(unix)]
async fn terminate() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[cfg(not(unix))]
async fn terminate() {
    let _ = tokio::signal::ctrl_c().await;
}

fn stop_on_signal() -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        terminate().await;
        let _ = tx.send(true);
    });
    rx
}

pub fn server(path: &Path, logging: Logging) -> Result<u8, Failure> {
    let Loaded::Server(settings, _) = load(path, Role::Server)? else {
        return Err(Failure::new(format!(
            "{} is not a server config",
            path.display()
        )));
    };
    init_logging(logging);
    runtime()?.block_on(async move {
        let server = Server::bind(*settings.clone()).await?;
        let stop = stop_on_signal();
        let (reload_tx, reload_rx) = mpsc::channel(4);
        tokio::spawn(reloader(path.to_path_buf(), *settings, reload_tx));
        server.run(reload_rx, stop).await;
        Ok(0)
    })
}

pub fn client(path: &Path, logging: Logging) -> Result<u8, Failure> {
    let Loaded::Client(settings, _) = load(path, Role::Client)? else {
        return Err(Failure::new(format!(
            "{} is not a client config",
            path.display()
        )));
    };
    init_logging(logging);
    runtime()?.block_on(async move {
        let stop = stop_on_signal();
        match Client::new(*settings).run(stop).await {
            RunEnd::Stopped => Ok(0),
            RunEnd::GaveUp(down) => Err(Failure::new(format!(
                "gave up after the server was unreachable for {}",
                crate::client::reconnect::human(down)
            ))
            .hint("raise or remove reconnect.give_up_after_secs to keep retrying")),
            RunEnd::Rejected => Err(Failure::new("the server rejected this client")
                .hint("check name and secret against the server config, or set reconnect.on_rejected: retry")),
        }
    })
}

pub async fn reloader(
    path: PathBuf,
    mut current: ServerSettings,
    tx: mpsc::Sender<ServerSettings>,
) {
    let (trig_tx, mut trig) = mpsc::channel::<&'static str>(16);
    let _watcher = watch_file(&path, trig_tx.clone());
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
    while let Some(why) = trig.recv().await {
        tokio::time::sleep(Duration::from_millis(200)).await;
        while trig.try_recv().is_ok() {}
        let outcome = match load_file(&path, Some(Role::Server), &ProcessEnv) {
            Ok(o) => o,
            Err(e) => {
                let (m, h) = e.describe(&path);
                error!("reload ({why}) rejected: {m} ({h}); keeping the current config");
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
            continue;
        }
        let Some(Loaded::Server(next, _)) = outcome.loaded else {
            continue;
        };
        if *next == current {
            debug!("config file changed but settings are the same");
            continue;
        }
        eprint!("{}", startup_report(&outcome.report, style));
        info!("reloading config ({why})");
        current = (*next).clone();
        if tx.send(*next).await.is_err() {
            return;
        }
    }
}

fn watch_file(path: &Path, tx: mpsc::Sender<&'static str>) -> Option<notify::RecommendedWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let file_name = path.file_name()?.to_os_string();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        let relevant = matches!(
            ev.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) && ev
            .paths
            .iter()
            .any(|p| p.file_name() == Some(file_name.as_os_str()));
        if relevant {
            let _ = tx.try_send("file changed");
        }
    })
    .map_err(|e| {
        warn!(
            "cannot watch {} for changes ({e}); reload with SIGHUP instead",
            path.display()
        )
    })
    .ok()?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|e| {
            warn!(
                "cannot watch {} for changes ({e}); reload with SIGHUP instead",
                dir.display()
            )
        })
        .ok()?;
    Some(watcher)
}
