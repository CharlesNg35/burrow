use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tracing::error;

use crate::client::{Client, RunEnd};
use crate::config::env::EnvSource;
use crate::config::{Loaded, Role, load_file};
use crate::diagnostics::Style;
use crate::failure::Failure;
use crate::instance::{self, Identity};
use crate::server::Server;
use crate::validate::startup_report;
use crate::visitor::{Pick, Visitor};

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
    pub stop: Option<watch::Receiver<bool>>,
    pub env: Env,
}

fn load(path: &Path, role: Role, env: &Env, quiet: &[&str]) -> Result<Loaded, Failure> {
    let mut outcome = load_file(path, Some(role), env).map_err(|e| {
        let (m, h) = e.describe(path);
        config_failure(Failure::new(m).hint(h))
    })?;
    outcome
        .report
        .diagnostics
        .retain(|d| d.is_error() || !quiet.contains(&d.code));
    let style = Style::detect(std::io::stderr().is_terminal());
    if let Some(d) = outcome
        .report
        .diagnostics
        .iter()
        .find(|d| d.code == "wrong-role")
    {
        let f = Failure::usage(d.message.clone());
        return Err(match &d.help {
            Some(h) => f.hint(h.clone()),
            None => f,
        });
    }
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
                "fix them, then check again with: {}",
                crate::failure::cmd(&format!("validate -c {}", crate::failure::path_arg(path)))
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

fn drive<T>(work: impl std::future::Future<Output = Result<T, Failure>>) -> Result<T, Failure> {
    let rt = runtime()?;
    let out = rt.block_on(work);
    rt.shutdown_timeout(std::time::Duration::from_secs(1));
    out
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

fn with_env(env: Env) -> Option<Hosted> {
    Some(Hosted { stop: None, env })
}

pub fn server(path: &Path, logging: Logging, env: Env) -> Result<u8, Failure> {
    server_until(path, logging, with_env(env))
}

pub fn client(path: &Path, logging: Logging, env: Env) -> Result<u8, Failure> {
    client_until(path, logging, with_env(env))
}

pub fn visit(path: &Path, logging: Logging, pick: Pick, env: Env) -> Result<u8, Failure> {
    raise_open_files();
    logged(
        &logging.clone(),
        run_visitor(path, logging, pick, with_env(env)),
    )
}

pub fn env_file(path: &Path) -> Result<Env, Failure> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        Failure::new(format!(
            "cannot read {}: {}",
            path.display(),
            crate::failure::file_reason(&e)
        ))
    })?;
    let vars: BTreeMap<String, String> = serde_json::from_str(&text)
        .map_err(|e| Failure::new(format!("{} is not a burrow env file: {e}", path.display())))?;
    Ok(Env::with(vars))
}

fn raise_open_files() {
    #[cfg(unix)]
    {
        use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
        let limit = getrlimit(Resource::Nofile);
        let most = limit.maximum.unwrap_or(1 << 20);
        if limit.current.is_some_and(|c| c >= most) {
            return;
        }
        for want in [most, most.min(10240)] {
            let raised = Rlimit {
                current: Some(want),
                maximum: limit.maximum,
            };
            if setrlimit(Resource::Nofile, raised).is_ok() {
                return;
            }
        }
    }
}

pub fn visit_until(path: &Path, logging: Logging, hosted: Option<Hosted>) -> Result<u8, Failure> {
    raise_open_files();
    logged(
        &logging.clone(),
        run_visitor(path, logging, Pick::All, hosted),
    )
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
    raise_open_files();
    logged(&logging.clone(), run_server(path, logging, hosted))
}

pub fn client_until(path: &Path, logging: Logging, hosted: Option<Hosted>) -> Result<u8, Failure> {
    raise_open_files();
    logged(&logging.clone(), run_client(path, logging, hosted))
}

fn split(hosted: Option<Hosted>) -> (Option<watch::Receiver<bool>>, Env) {
    match hosted {
        Some(h) => (h.stop, h.env),
        None => (None, Env::default()),
    }
}

fn run_server(path: &Path, logging: Logging, hosted: Option<Hosted>) -> Result<u8, Failure> {
    let (stop, env) = split(hosted);
    let Loaded::Server(settings, _) = load(path, Role::Server, &env, &[])? else {
        return Err(Failure::new(format!(
            "{} is not a server config",
            path.display()
        )));
    };
    let _instance = instance::acquire(&Identity::server(path), path)?;
    init_logging(&logging)?;
    drive(async move {
        let server = Server::bind(*settings.clone()).await?;
        let shared = server.shared();
        let _status = crate::status::publish(path, Arc::new(move || shared.status()));
        let stop = stop_on_signal(stop);
        let (reload_tx, reload_rx) = mpsc::channel(4);
        tokio::spawn(crate::reload::follow(
            path.to_path_buf(),
            *settings,
            env,
            crate::reload::accept(),
            reload_tx,
        ));
        server.run(reload_rx, stop).await;
        Ok(0)
    })
}

fn run_client(path: &Path, logging: Logging, hosted: Option<Hosted>) -> Result<u8, Failure> {
    let (stop, env) = split(hosted);
    let Loaded::Client(settings, _) = load(path, Role::Client, &env, &[])? else {
        return Err(Failure::new(format!(
            "{} is not a client config",
            path.display()
        )));
    };
    let identity = Identity::client(&settings.endpoint.to_string(), &settings.name);
    let lock = instance::acquire(&identity, path)?;
    init_logging(&logging)?;
    drive(async move {
        let stop = stop_on_signal(stop);
        let (reload_tx, reload_rx) = mpsc::channel(4);
        tokio::spawn(crate::reload::follow(
            path.to_path_buf(),
            (*settings).clone(),
            env,
            same_tunnel_once(path.to_path_buf(), identity, lock),
            reload_tx,
        ));
        match Client::new(*settings).with_reloads(reload_rx).run(stop).await {
            RunEnd::Stopped => Ok(0),
            RunEnd::GaveUp(down, why) => Err(Failure::new(format!(
                "gave up after {} without a working connection to the server; last error: {why}",
                crate::client::reconnect::human(down)
            ))
            .hint("raise or remove reconnect.give_up_after_secs to keep retrying")),
            RunEnd::Rejected => Err(Failure::new("the server rejected this client")
                .hint("check name and secret against the server config, or set reconnect.on_rejected: retry")),
        }
    })
}

fn run_visitor(
    path: &Path,
    logging: Logging,
    pick: Pick,
    hosted: Option<Hosted>,
) -> Result<u8, Failure> {
    let (stop, env) = split(hosted);
    let quiet: &[&str] = match pick {
        Pick::Adhoc { .. } => &["no-forwards"],
        _ => &[],
    };
    let Loaded::Visitor(mut settings, _) = load(path, Role::Visitor, &env, quiet)? else {
        return Err(Failure::new(format!(
            "{} is not a visitor config",
            path.display()
        )));
    };
    let shown = path.display().to_string();
    crate::visitor::pick(&mut settings, pick.clone(), &shown)?;
    init_logging(&logging)?;
    drive(async move {
        let (reload_tx, reload_rx) = mpsc::channel(4);
        tokio::spawn(crate::reload::follow(
            path.to_path_buf(),
            (*settings).clone(),
            env,
            Box::new(move |mut next| {
                crate::visitor::pick(&mut next, pick.clone(), &shown).map_err(|f| f.message)?;
                Ok(next)
            }),
            reload_tx,
        ));
        let visitor = Visitor::bind(*settings)?.with_reloads(reload_rx);
        let stop = stop_on_signal(stop);
        visitor.run(stop).await;
        Ok(0)
    })
}

fn same_tunnel_once(
    path: PathBuf,
    identity: Identity,
    lock: instance::InstanceLock,
) -> crate::reload::Check<crate::config::settings::ClientSettings> {
    let mut held = (identity, lock);
    Box::new(move |next| {
        let wanted = Identity::client(&next.endpoint.to_string(), &next.name);
        if held.0 != wanted {
            let lock = instance::acquire(&wanted, &path).map_err(|f| f.message)?;
            held = (wanted, lock);
        }
        Ok(next)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_does_not_wait_for_a_stuck_blocking_lookup() {
        let started = std::time::Instant::now();
        let out = drive(async {
            tokio::task::spawn_blocking(|| std::thread::sleep(std::time::Duration::from_secs(20)));
            Ok(7)
        });
        assert_eq!(out.unwrap(), 7);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
