use std::path::Path;
use std::sync::Mutex;

use burrow_admin_proto::updates::{
    RemoteUpdates, RunMode, ServerUpdate, UpdateState, UpdateStatus,
};

use super::Mode;
use super::record::{self, Asked, Record};
use super::service_side::{WATCH_FOR, answers, asked_file};
use crate::config::settings::ServerSettings;
use crate::install::{Manifest, Role};
use crate::service::helper;
use crate::wire::protocol::Outcome;

pub struct ServerUpdates {
    mode: Mode,
    asking: Mutex<()>,
}

impl ServerUpdates {
    pub fn new(config: &Path) -> ServerUpdates {
        let config = crate::base::fs::canonical_or_same(config);
        ServerUpdates {
            mode: Mode::detect(&config, Role::Server),
            asking: Mutex::new(()),
        }
    }
}

pub fn off_reason(m: &Manifest, written: bool) -> String {
    let install = crate::base::failure::admin(&format!(
        "service install -c {}",
        crate::base::failure::path_arg(&m.source_config)
    ));
    match written {
        true => format!(
            "server.yaml has update.remote: true, but it counts only once root installs the service again: {install}"
        ),
        false => format!(
            "remote updates are off: turn them on with update.remote: true in server.yaml, then {install}"
        ),
    }
}

pub(super) fn remote(mode: &Mode, written: bool) -> (RemoteUpdates, Option<String>) {
    let blocked = |why: String| (RemoteUpdates::Blocked, Some(why));
    match mode {
        Mode::Container => blocked("it runs in a container: update the image instead".into()),
        Mode::ByHand { .. } => blocked(format!(
            "it was started by hand, not as a service: update it with {}",
            crate::base::failure::admin("update")
        )),
        Mode::Service(m) if !m.remote_updates || helper::helper().is_none() => {
            (RemoteUpdates::Off, Some(off_reason(m, written)))
        }
        Mode::Service(_) => match mode.blocked() {
            Some(why) => blocked(why),
            None => (RemoteUpdates::On, None),
        },
    }
}

fn run(mode: &Mode) -> RunMode {
    match mode {
        Mode::ByHand { .. } => RunMode::ByHand,
        Mode::Service(_) => RunMode::Service,
        Mode::Container => RunMode::Container,
    }
}

fn state_of(o: Outcome) -> UpdateState {
    match o {
        Outcome::Done => UpdateState::Done,
        Outcome::RolledBack => UpdateState::RolledBack,
        Outcome::Trying => UpdateState::Restarting,
        Outcome::Failed => UpdateState::Failed,
    }
}

pub fn last(result: Option<Record>, asked: Option<Asked>, now: u64) -> Option<UpdateStatus> {
    let ago = |at: u64| now.saturating_sub(at);
    match (result, asked) {
        (Some(r), Some(a)) if answers(&r, &a) => Some(UpdateStatus {
            from: r.from.clone(),
            to: r.to.clone(),
            state: r.outcome().map_or(UpdateState::Failed, state_of),
            detail: r.detail.clone(),
            by: a.by,
            secs_ago: ago(r.at),
            ..UpdateStatus::default()
        }),
        (_, Some(a)) if ago(a.at) < WATCH_FOR.as_secs() => Some(UpdateStatus {
            from: a.from.clone(),
            state: UpdateState::Asked,
            by: a.by,
            secs_ago: ago(a.at),
            ..UpdateStatus::default()
        }),
        (_, Some(a)) => Some(UpdateStatus {
            from: a.from.clone(),
            state: UpdateState::TimedOut,
            detail: "the updater did not answer".into(),
            by: a.by,
            secs_ago: ago(a.at),
            ..UpdateStatus::default()
        }),
        (Some(r), None) => Some(UpdateStatus {
            from: r.from.clone(),
            to: r.to.clone(),
            state: r.outcome().map_or(UpdateState::Failed, state_of),
            detail: r.detail.clone(),
            secs_ago: ago(r.at),
            ..UpdateStatus::default()
        }),
        (None, None) => None,
    }
}

impl crate::admin::own_update::OwnUpdate for ServerUpdates {
    fn state(&self, settings: &ServerSettings) -> ServerUpdate {
        let (remote, why_not) = remote(&self.mode, settings.remote_updates);
        let last = match &self.mode {
            Mode::Service(m) => last(
                record::read(&helper::result_file(m)),
                record::read(&asked_file(m)),
                record::now(),
            ),
            _ => None,
        };
        ServerUpdate {
            run: run(&self.mode),
            remote,
            why_not,
            last,
        }
    }

    fn ask(&self, settings: &ServerSettings, by: &str) -> Result<(), String> {
        let _one = self.asking.lock().expect("asking lock");
        let now = self.state(settings);
        if let Some(why) = now.why_not.filter(|_| now.remote != RemoteUpdates::On) {
            return Err(why);
        }
        if now.last.as_ref().is_some_and(|l| !l.state.finished()) {
            return Err("an update of this server is already running".into());
        }
        let Mode::Service(m) = &self.mode else {
            return Err("only a service updates itself this way".into());
        };
        let asked = Asked {
            id: 0,
            at: record::now(),
            from: crate::base::build::version().to_string(),
            by: by.to_string(),
        };
        let file = asked_file(m);
        record::write(&file, &asked, 0o600)
            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        helper::fire(m)?;
        tracing::info!(
            "{by} asked to update this server; {} downloads the latest release and restarts it",
            helper::helper_name(m)
        );
        Ok(())
    }
}
