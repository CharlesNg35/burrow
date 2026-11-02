mod by_hand;
pub mod helper;
mod record;
mod service_side;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use crate::client::updates::{Progress, Updates};
use crate::config::settings::ClientSettings;
use crate::install::Manifest;
use crate::wire::protocol::{LastUpdate, Outcome, Remote, Report, RunMode};
pub use record::{Asked, Record};

pub const WINDOW: Duration = Duration::from_secs(60);
pub const MAX_STARTS: u32 = 3;
pub const WINDOW_ENV: &str = "BURROW_ADMIN_TEST_UPDATE_WINDOW";

pub fn window() -> Duration {
    if !cfg!(feature = "admin-test-hooks") {
        return WINDOW;
    }
    std::env::var(WINDOW_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(WINDOW, Duration::from_secs)
}

pub fn allowed_target(target: &str, mine: &str) -> Result<semver::Version, String> {
    let target = target.trim().trim_start_matches('v');
    let wanted = super::release_version(target)
        .ok_or_else(|| format!("\"{target}\" is not a release number like 1.2.3"))?;
    let ours = crate::service::parse_version(mine)
        .ok_or_else(|| format!("this burrow reports \"{mine}\", which is not a release number"))?;
    if wanted <= ours {
        return Err(format!(
            "{target} is not newer than {mine}, and updates never go back"
        ));
    }
    if !wanted.pre.is_empty() && ours.pre.is_empty() {
        return Err(format!(
            "{target} is a pre-release; only a client that already runs one takes it"
        ));
    }
    Ok(wanted)
}

pub enum Mode {
    ByHand { binary: Option<PathBuf> },
    Service(Box<Manifest>),
    Container,
}

impl Mode {
    pub fn detect(config: &Path) -> Mode {
        if super::in_container() {
            return Mode::Container;
        }
        let me = crate::service::this_binary().ok();
        let config = crate::base::fs::canonical_or_same(config);
        let mine = crate::install::all_manifests().into_iter().find(|m| {
            m.role == crate::install::Role::Client
                && crate::base::fs::canonical_or_same(&m.config) == config
                && me.as_ref().is_some_and(|b| *b == m.binary)
        });
        match mine {
            Some(m) => Mode::Service(Box::new(m)),
            None => Mode::ByHand { binary: me },
        }
    }

    pub fn run(&self) -> RunMode {
        match self {
            Mode::ByHand { .. } => RunMode::ByHand,
            Mode::Service(_) => RunMode::Service,
            Mode::Container => RunMode::Container,
        }
    }

    pub fn blocked(&self) -> Option<String> {
        if crate::base::build::release_asset().is_none() {
            return Some("there is no release build for this system".into());
        }
        match self {
            Mode::Container => Some("it runs in a container: update the image instead".into()),
            Mode::Service(m) => service_side::blocked(m),
            Mode::ByHand { binary: None } => Some("it cannot find its own program file".into()),
            Mode::ByHand { binary: Some(b) } => by_hand::shared_with_services(b).or_else(|| {
                (!by_hand::can_replace(b))
                    .then(|| format!("this account cannot replace {}", b.display()))
            }),
        }
    }
}

pub struct ClientUpdates {
    config: PathBuf,
    mode: Mode,
    busy: Arc<AtomicBool>,
    stop: Arc<watch::Sender<bool>>,
    after: Arc<Mutex<Option<PathBuf>>>,
    trial: Arc<Mutex<Option<Record>>>,
    report_again: Arc<std::sync::OnceLock<Arc<watch::Sender<u64>>>>,
}

impl ClientUpdates {
    pub fn new(config: &Path) -> ClientUpdates {
        let config = crate::base::fs::canonical_or_same(config);
        let mode = Mode::detect(&config);
        ClientUpdates {
            config,
            mode,
            busy: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(watch::Sender::new(false)),
            after: Arc::new(Mutex::new(None)),
            trial: Arc::new(Mutex::new(None)),
            report_again: Arc::default(),
        }
    }

    fn last(&self) -> Option<LastUpdate> {
        match &self.mode {
            Mode::ByHand { binary: Some(me) } => record::read::<Record>(&by_hand::record_path(me))
                .filter(|r| r.config == self.config)
                .and_then(|r| r.last()),
            Mode::Service(m) => service_side::last(m),
            _ => None,
        }
    }

    fn by_hand(&self) -> Option<by_hand::Shared> {
        let Mode::ByHand { binary: Some(me) } = &self.mode else {
            return None;
        };
        Some(by_hand::Shared {
            me: me.clone(),
            config: self.config.clone(),
            busy: self.busy.clone(),
            stop: self.stop.clone(),
            after: self.after.clone(),
            trial: self.trial.clone(),
        })
    }
}

impl Updates for ClientUpdates {
    fn report(&self, settings: &ClientSettings) -> Report {
        let remote = if !settings.remote_updates {
            Remote::Off
        } else {
            match self.mode.blocked() {
                Some(why) => Remote::Blocked(why),
                None => Remote::On,
            }
        };
        Report {
            version: crate::base::build::version().to_string(),
            os: crate::base::build::os().to_string(),
            arch: crate::base::build::arch().to_string(),
            run: self.mode.run(),
            remote,
            last: self.last(),
        }
    }

    fn connected(&self) {
        if let Some(shared) = self.by_hand() {
            shared.confirm();
        }
        if let (Mode::Service(m), Some(again)) = (&self.mode, self.report_again.get()) {
            service_side::await_result(m, again.clone());
        }
    }

    fn attach(&self, report_again: Arc<watch::Sender<u64>>) {
        let _ = self.report_again.set(report_again);
    }

    fn ask(
        &self,
        settings: &ClientSettings,
        id: u32,
        version: &str,
        progress: Progress,
    ) -> Result<(), String> {
        if !settings.remote_updates {
            return Err("remote updates are off in its config".into());
        }
        if let Some(why) = self.mode.blocked() {
            return Err(why);
        }
        let mine = crate::base::build::version();
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err("an update is already running".into());
        }
        let job = Job {
            id,
            from: mine.to_string(),
            to: version.trim().to_string(),
            progress,
        };
        let started = match (&self.mode, self.by_hand()) {
            (_, Some(shared)) => {
                by_hand::start(shared, job);
                Ok(())
            }
            (Mode::Service(m), _) => service_side::start(m, job, self.busy.clone()),
            _ => Err("this client cannot update itself".into()),
        };
        if started.is_err() {
            self.busy.store(false, Ordering::Release);
        }
        started
    }

    fn stop_wanted(&self) -> Option<watch::Receiver<bool>> {
        self.by_hand().map(|_| self.stop.subscribe())
    }

    fn before_start(&self) -> Option<Result<(), String>> {
        self.by_hand()?.begin()
    }

    fn after_stop(&self) -> Option<Result<u8, String>> {
        let me = self.after.lock().expect("after lock").take()?;
        Some(by_hand::restart(&me))
    }
}

pub struct Job {
    pub id: u32,
    pub from: String,
    pub to: String,
    pub progress: Progress,
}

impl Job {
    fn say(&self, step: crate::wire::protocol::Step, detail: impl Into<String>) {
        let _ = self.progress.send((self.id, step, detail.into()));
    }

    fn record(&self, config: &Path) -> Record {
        Record::new(self.id, &self.from, &self.to, config)
    }
}

pub fn outcome_of(message: &str) -> Outcome {
    if message.contains("restored") {
        Outcome::RolledBack
    } else {
        Outcome::Failed
    }
}

#[cfg(test)]
mod tests;
