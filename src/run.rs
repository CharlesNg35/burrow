use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use crate::client::{Client, RunEnd};
use crate::config::env::EnvSource;
use crate::config::settings::ServerSettings;
use crate::config::{Loaded, Role, load_file};
use crate::diagnostics::Style;
use crate::failure::Failure;
use crate::instance::{self, Identity};
use crate::server::Server;
use crate::validate::startup_report;

#[derive(Debug, Clone)]
pub struct Logging {
    pub level: tracing::Level,
    pub json: bool,
    pub file: Option<PathBuf>,
}

pub fn init_logging(l: &Logging) -> Result<(), Failure> {
    let filter = tracing_subscriber::EnvFilter::try_from_env("BURROW_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(format!("burrow={}", l.level)));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    let result = match (&l.file, l.json) {
        (Some(path), json) => {
            let file = crate::logfile::RotatingFile::open(path).map_err(|e| {
                Failure::new(format!(
                    "cannot open log file {}: {}",
                    path.display(),
                    crate::failure::file_reason(&e)
                ))
            })?;
            let builder = builder.with_writer(file).with_ansi(false);
            if json {
                builder.json().flatten_event(true).try_init()
            } else {
                builder.try_init()
            }
        }
        (None, true) => builder
            .with_writer(std::io::stderr)
            .json()
            .flatten_event(true)
            .try_init(),
        (None, false) => {
            let color = Style::detect(std::io::stderr().is_terminal()).color;
            builder
                .with_writer(std::io::stderr)
                .with_ansi(color)
                .try_init()
        }
    };
    let _ = result;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct Env(Arc<BTreeMap<String, String>>);

impl Env {
    pub fn with(vars: BTreeMap<String, String>) -> Env {
        Env(Arc::new(vars))
    }
}

impl EnvSource for Env {
    fn get(&self, name: &str) -> Option<String> {
        self.0
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
    }
}

pub struct Hosted {
    pub stop: watch::Receiver<bool>,
    pub env: Env,
}

fn load(path: &Path, role: Role, env: &Env) -> Result<Loaded, Failure> {
    let outcome = load_file(path, Some(role), env).map_err(|e| {
        let (m, h) = e.describe(path);
        config_failure(Failure::new(m).hint(h))
    })?;
    let style = Style::detect(std::io::stderr().is_terminal());
    if outcome.report.has_errors() {
        eprint!("{}", outcome.report.render_text(style));
        return Err(config_failure(
            Failure::new(format!(
                "{} has {} {}, not starting",
                path.display(),
                outcome.report.error_count(),
                crate::diagnostics::plural(outcome.report.error_count(), "error")
            ))
            .hint(format!(
                "fix them, then check again with: {} validate -c {}",
                crate::failure::command(),
                path.display()
            )),
        ));
    }
    eprint!("{}", startup_report(&outcome.report, style));
    outcome
        .loaded
        .ok_or_else(|| Failure::new(format!("{} could not be loaded", path.display())))
}

fn config_failure(mut f: Failure) -> Failure {
    f.exit = crate::failure::EXIT_CONFIG;
    f
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
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn stop_on_signal(external: Option<watch::Receiver<bool>>) -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        match external {
            Some(mut ext) => {
                tokio::select! {
                    _ = terminate() => {}
                    _ = ext.wait_for(|stop| *stop) => {}
                }
            }
            None => terminate().await,
        }
        let _ = tx.send(true);
    });
    rx
}

pub fn server(path: &Path, logging: Logging) -> Result<u8, Failure> {
    server_until(path, logging, None)
}

pub fn client(path: &Path, logging: Logging) -> Result<u8, Failure> {
    client_until(path, logging, None)
}

fn logged(logging: &Logging, result: Result<u8, Failure>) -> Result<u8, Failure> {
    if let (Err(f), Some(_)) = (&result, &logging.file) {
        let _ = init_logging(logging);
        match &f.hint {
            Some(h) => error!("{} ({h})", f.message),
            None => error!("{}", f.message),
        }
    }
    result
}

pub fn server_until(path: &Path, logging: Logging, hosted: Option<Hosted>) -> Result<u8, Failure> {
    logged(&logging.clone(), run_server(path, logging, hosted))
}

pub fn client_until(path: &Path, logging: Logging, hosted: Option<Hosted>) -> Result<u8, Failure> {
    logged(&logging.clone(), run_client(path, logging, hosted))
}

fn split(hosted: Option<Hosted>) -> (Option<watch::Receiver<bool>>, Env) {
    match hosted {
        Some(h) => (Some(h.stop), h.env),
        None => (None, Env::default()),
    }
}

fn run_server(path: &Path, logging: Logging, hosted: Option<Hosted>) -> Result<u8, Failure> {
    let (stop, env) = split(hosted);
    let Loaded::Server(settings, _) = load(path, Role::Server, &env)? else {
        return Err(Failure::new(format!(
            "{} is not a server config",
            path.display()
        )));
    };
    let _instance = instance::acquire(&Identity::server(path), path)?;
    init_logging(&logging)?;
    runtime()?.block_on(async move {
        let server = Server::bind(*settings.clone()).await?;
        let stop = stop_on_signal(stop);
        let (reload_tx, reload_rx) = mpsc::channel(4);
        tokio::spawn(reloader(path.to_path_buf(), *settings, env, reload_tx));
        server.run(reload_rx, stop).await;
        Ok(0)
    })
}

fn run_client(path: &Path, logging: Logging, hosted: Option<Hosted>) -> Result<u8, Failure> {
    let (stop, env) = split(hosted);
    let Loaded::Client(settings, _) = load(path, Role::Client, &env)? else {
        return Err(Failure::new(format!(
            "{} is not a client config",
            path.display()
        )));
    };
    let identity = Identity::client(&settings.endpoint.to_string(), &settings.name);
    let _instance = instance::acquire(&identity, path)?;
    init_logging(&logging)?;
    runtime()?.block_on(async move {
        let stop = stop_on_signal(stop);
        ignore_hangup_on_client();
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

#[cfg(unix)]
fn ignore_hangup_on_client() {
    tokio::spawn(async {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut hup) = signal(SignalKind::hangup()) else {
            return;
        };
        while hup.recv().await.is_some() {
            info!(
                "a client applies config changes when it restarts; restart it to use the edited file"
            );
        }
    });
}

#[cfg(not(unix))]
fn ignore_hangup_on_client() {}

pub async fn reloader(
    path: PathBuf,
    mut current: ServerSettings,
    env: Env,
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
        let outcome = match load_file(&path, Some(Role::Server), &env) {
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
        let Some(Loaded::Server(next, _)) = outcome.loaded else {
            continue;
        };
        let was_rejected = std::mem::take(&mut rejected);
        if *next == current && !asked {
            if was_rejected {
                info!("config is valid again and matches the running config");
            } else {
                debug!("config file changed but settings are the same");
            }
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
