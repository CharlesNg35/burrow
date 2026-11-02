use std::collections::HashMap;
use std::time::{Duration, Instant};

use burrow_admin_proto::updates::{ClientBuild, RolloutStatus, UpdateState, UpdateStatus};
use tracing::{info, warn};

use crate::wire::protocol::{Outcome, Remote, Report, ServerMsg, Step};

const ANSWER: Duration = Duration::from_secs(30);
const COME_BACK: Duration = Duration::from_secs(15 * 60);

struct Job {
    id: u32,
    from: String,
    to: String,
    by: String,
    state: UpdateState,
    detail: String,
    changed: Instant,
}

impl Job {
    fn set(&mut self, state: UpdateState, detail: impl Into<String>, now: Instant) {
        self.state = state;
        self.detail = detail.into();
        self.changed = now;
    }

    fn shown(&self, now: Instant) -> UpdateStatus {
        UpdateStatus {
            id: self.id,
            from: self.from.clone(),
            to: self.to.clone(),
            state: self.state,
            detail: self.detail.clone(),
            by: self.by.clone(),
            secs_ago: now.duration_since(self.changed).as_secs(),
        }
    }
}

struct Rollout {
    to: String,
    by: String,
    clients: Vec<String>,
    next: usize,
    current: Option<String>,
    done: Vec<String>,
    stopped_at: Option<String>,
}

impl Rollout {
    fn finished(&self) -> bool {
        self.stopped_at.is_some() || (self.current.is_none() && self.next >= self.clients.len())
    }
}

pub(crate) type Ask = (String, u32, String);

#[derive(Default)]
pub(crate) struct Board {
    reports: HashMap<String, Report>,
    jobs: HashMap<String, Job>,
    next_id: u32,
    rollout: Option<Rollout>,
}

impl Board {
    pub(crate) fn reported(&mut self, client: &str, report: Report, now: Instant) {
        if let Some(job) = self.jobs.get_mut(client).filter(|j| !j.state.finished()) {
            if report.version == job.to {
                job.set(UpdateState::Done, "", now);
                info!("client \"{client}\" updated {} -> {}", job.from, job.to);
            } else if let Some(last) = report.last.as_ref().filter(|l| l.id == job.id) {
                let state = match last.outcome {
                    Outcome::RolledBack => Some(UpdateState::RolledBack),
                    Outcome::Failed => Some(UpdateState::Failed),
                    Outcome::Done | Outcome::Trying => None,
                };
                if let Some(state) = state {
                    job.set(state, last.detail.clone(), now);
                    warn!(
                        "client \"{client}\" did not update {} -> {}: {}",
                        job.from, job.to, last.detail
                    );
                }
            }
        }
        self.reports.insert(client.to_string(), report);
    }

    pub(crate) fn seated(&mut self, client: &str) {
        self.reports.remove(client);
    }

    pub(crate) fn build(&self, client: &str) -> Option<ClientBuild> {
        self.reports.get(client).map(Report::build)
    }

    pub(crate) fn job(&self, client: &str, now: Instant) -> Option<UpdateStatus> {
        self.jobs.get(client).map(|j| j.shown(now))
    }

    pub(crate) fn begin(
        &mut self,
        client: &str,
        to: &str,
        by: &str,
        now: Instant,
    ) -> Result<u32, String> {
        let report = self.reports.get(client).ok_or_else(|| {
            format!(
                "it does not report its version, so it is offline or older than {}",
                burrow_admin_proto::updates::FIRST_REPORTING
            )
        })?;
        if report.remote != Remote::On {
            return Err("it does not allow remote updates".into());
        }
        if self.jobs.get(client).is_some_and(|j| !j.state.finished()) {
            return Err("an update is already running".into());
        }
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.jobs.insert(
            client.to_string(),
            Job {
                id: self.next_id,
                from: report.version.clone(),
                to: to.to_string(),
                by: by.to_string(),
                state: UpdateState::Asked,
                detail: String::new(),
                changed: now,
            },
        );
        info!(
            "{by} asked client \"{client}\" to update {} -> {to}",
            report.version
        );
        Ok(self.next_id)
    }

    pub(crate) fn forget(&mut self, client: &str, id: u32) {
        if self.jobs.get(client).is_some_and(|j| j.id == id) {
            self.jobs.remove(client);
        }
    }

    pub(crate) fn stepped(
        &mut self,
        client: &str,
        id: u32,
        step: Step,
        detail: String,
        now: Instant,
    ) {
        let Some(job) = self
            .jobs
            .get_mut(client)
            .filter(|j| j.id == id && !j.state.finished())
        else {
            return;
        };
        let state = match step {
            Step::Accepted => UpdateState::Downloading,
            Step::Restarting => UpdateState::Restarting,
            Step::Refused => UpdateState::Refused,
            Step::Failed => UpdateState::Failed,
        };
        if state.failed() {
            warn!(
                "client \"{client}\" did not update {} -> {}: {detail}",
                job.from, job.to
            );
        }
        job.set(state, detail, now);
    }

    pub(crate) fn start_rollout(
        &mut self,
        clients: Vec<String>,
        to: &str,
        by: &str,
    ) -> Result<(), String> {
        if self.rollout.as_ref().is_some_and(|r| !r.finished()) {
            return Err("an update of several clients is already running".into());
        }
        if clients.is_empty() {
            return Err("no client to update".into());
        }
        info!(
            "{by} asked to update {} to {to}, one at a time",
            clients.join(", ")
        );
        self.rollout = Some(Rollout {
            to: to.to_string(),
            by: by.to_string(),
            clients,
            next: 0,
            current: None,
            done: Vec::new(),
            stopped_at: None,
        });
        Ok(())
    }

    pub(crate) fn rollout(&self) -> Option<RolloutStatus> {
        self.rollout.as_ref().map(|r| RolloutStatus {
            to: r.to.clone(),
            by: r.by.clone(),
            clients: r.clients.clone(),
            current: r.current.clone(),
            done: r.done.clone(),
            stopped_at: r.stopped_at.clone(),
            finished: r.finished(),
        })
    }

    pub(crate) fn tick(&mut self, now: Instant) -> Vec<Ask> {
        for (client, job) in &mut self.jobs {
            let waited = now.duration_since(job.changed);
            let late = match job.state {
                UpdateState::Asked => waited > ANSWER,
                UpdateState::Downloading | UpdateState::Restarting => waited > COME_BACK,
                _ => false,
            };
            if late {
                let why = format!("no word from it for {} s", waited.as_secs());
                warn!(
                    "client \"{client}\" did not update {} -> {}: {why}",
                    job.from, job.to
                );
                job.set(UpdateState::TimedOut, why, now);
            }
        }
        self.advance(now)
    }

    fn advance(&mut self, now: Instant) -> Vec<Ask> {
        let mut asks = Vec::new();
        let Some(mut r) = self.rollout.take() else {
            return asks;
        };
        loop {
            if let Some(c) = r.current.clone() {
                match self.jobs.get(&c).map(|j| j.state) {
                    Some(UpdateState::Done) => {
                        r.done.push(c);
                        r.current = None;
                    }
                    Some(s) if s.finished() => {
                        warn!("stopped updating to {} at client \"{c}\"", r.to);
                        r.stopped_at = Some(c);
                    }
                    _ => break,
                }
            }
            if r.finished() {
                break;
            }
            let c = r.clients[r.next].clone();
            r.next += 1;
            match self.begin(&c, &r.to, &r.by, now) {
                Ok(id) => {
                    asks.push((c.clone(), id, r.to.clone()));
                    r.current = Some(c);
                }
                Err(why) => {
                    warn!("stopped updating to {} at client \"{c}\": {why}", r.to);
                    r.stopped_at = Some(c);
                }
            }
        }
        self.rollout = Some(r);
        asks
    }
}

impl super::Shared {
    pub(super) fn reported(&self, client: &str, report: Report) {
        tracing::debug!(
            "client \"{client}\" runs burrow {} ({}/{}, {})",
            report.version,
            report.os,
            report.arch,
            report.run.wire()
        );
        let mut board = self.updates.lock().expect("updates lock");
        board.reported(client, report, Instant::now());
        let asks = board.tick(Instant::now());
        drop(board);
        self.send_asks(asks);
    }

    pub(super) fn stepped(&self, client: &str, id: u32, step: Step, detail: String) {
        let mut board = self.updates.lock().expect("updates lock");
        board.stepped(client, id, step, detail, Instant::now());
        let asks = board.tick(Instant::now());
        drop(board);
        self.send_asks(asks);
    }

    pub(super) fn seated(&self, client: &str) {
        self.updates.lock().expect("updates lock").seated(client);
    }

    pub(super) fn build_of(&self, client: &str) -> Option<ClientBuild> {
        self.updates.lock().expect("updates lock").build(client)
    }

    pub(super) fn job_of(&self, client: &str) -> Option<UpdateStatus> {
        self.updates
            .lock()
            .expect("updates lock")
            .job(client, Instant::now())
    }

    pub(super) fn rollout(&self) -> Option<RolloutStatus> {
        self.updates.lock().expect("updates lock").rollout()
    }

    pub(super) fn drive_updates(&self) {
        let asks = self
            .updates
            .lock()
            .expect("updates lock")
            .tick(Instant::now());
        self.send_asks(asks);
    }

    pub fn ask_update(&self, client: &str, to: &str, by: &str) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .expect("sessions lock")
            .get(client)
            .cloned()
            .ok_or("it is offline")?;
        let id =
            self.updates
                .lock()
                .expect("updates lock")
                .begin(client, to, by, Instant::now())?;
        let sent = session.control.try_send(ServerMsg::Update {
            id,
            version: to.to_string(),
        });
        if sent.is_err() {
            self.updates
                .lock()
                .expect("updates lock")
                .forget(client, id);
            return Err("its connection is busy; try again".into());
        }
        Ok(())
    }

    pub fn update_all(&self, clients: Vec<String>, to: &str, by: &str) -> Result<(), String> {
        let mut board = self.updates.lock().expect("updates lock");
        board.start_rollout(clients, to, by)?;
        let asks = board.tick(Instant::now());
        drop(board);
        self.send_asks(asks);
        Ok(())
    }

    fn send_asks(&self, asks: Vec<Ask>) {
        for (client, id, version) in asks {
            let session = self
                .sessions
                .lock()
                .expect("sessions lock")
                .get(&client)
                .cloned();
            let sent = session.is_some_and(|s| {
                s.control
                    .try_send(ServerMsg::Update {
                        id,
                        version: version.clone(),
                    })
                    .is_ok()
            });
            if !sent {
                self.stepped(&client, id, Step::Failed, "it went offline".into());
            }
        }
    }
}

#[cfg(test)]
mod tests;
