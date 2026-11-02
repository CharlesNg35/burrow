use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;
use tracing::{info, warn};

use super::record::{self, Record};
use super::{Job, MAX_STARTS, window};
use crate::wire::protocol::{Outcome, Step};

pub struct Shared {
    pub me: PathBuf,
    pub config: PathBuf,
    pub busy: Arc<AtomicBool>,
    pub stop: Arc<watch::Sender<bool>>,
    pub after: Arc<Mutex<Option<PathBuf>>>,
    pub trial: Arc<Mutex<Option<Record>>>,
}

pub fn record_path(me: &Path) -> PathBuf {
    me.with_extension("update.json")
}

pub fn previous_path(me: &Path) -> PathBuf {
    me.with_extension("previous")
}

pub fn can_replace(binary: &Path) -> bool {
    let Some(dir) = binary.parent() else {
        return false;
    };
    let probe = dir.join(format!(".burrow-write-check-{}", std::process::id()));
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    let _ = std::fs::remove_file(&probe);
    made
}

pub fn trial_of(me: &Path, config: &Path) -> Option<Record> {
    let path = record_path(me);
    let mut r: Record = record::read(&path)?;
    let mine = crate::base::build::version();
    if r.config != config || r.outcome() != Some(Outcome::Trying) || r.to != mine {
        return None;
    }
    r.starts += 1;
    let _ = record::write(&path, &r, 0o600);
    Some(r)
}

fn text(f: crate::base::failure::Failure) -> String {
    f.message
}

impl Shared {
    pub fn begin(&self) -> Option<Result<u8, String>> {
        let r = self.trial.lock().expect("trial lock").clone()?;
        if r.starts >= MAX_STARTS {
            let why = format!(
                "{} stopped {} times before it connected to the server",
                r.to,
                r.starts - 1
            );
            if let Err(e) = self.roll_back(&why) {
                return Some(Err(e));
            }
            return Some(restart(&self.me));
        }
        let me = self.clone_shared();
        std::thread::spawn(move || {
            std::thread::sleep(window());
            me.expire();
        });
        None
    }

    fn clone_shared(&self) -> Shared {
        Shared {
            me: self.me.clone(),
            config: self.config.clone(),
            busy: self.busy.clone(),
            stop: self.stop.clone(),
            after: self.after.clone(),
            trial: self.trial.clone(),
        }
    }

    fn expire(&self) {
        let Some(r) = self.trial.lock().expect("trial lock").clone() else {
            return;
        };
        let why = format!(
            "{} did not connect to the server within {} s",
            r.to,
            window().as_secs()
        );
        match self.roll_back(&why) {
            Ok(()) => {
                *self.after.lock().expect("after lock") = Some(self.me.clone());
                self.stop.send_replace(true);
            }
            Err(e) => warn!("cannot go back to {}: {e}", r.from),
        }
    }

    fn roll_back(&self, why: &str) -> Result<(), String> {
        let mut trial = self.trial.lock().expect("trial lock");
        let Some(r) = trial.take() else {
            return Ok(());
        };
        warn!("{why}; going back to {}", r.from);
        crate::service::copy_file(&previous_path(&self.me), &self.me, true).map_err(text)?;
        let ended = r.ended(Outcome::RolledBack, why);
        record::write(&record_path(&self.me), &ended, 0o600)
            .map_err(|e| format!("cannot write {}: {e}", record_path(&self.me).display()))
    }

    pub fn confirm(&self) {
        let Some(r) = self.trial.lock().expect("trial lock").take() else {
            return;
        };
        info!("updated {} -> {}: connected to the server", r.from, r.to);
        let _ = record::write(&record_path(&self.me), &r.ended(Outcome::Done, ""), 0o600);
    }
}

pub fn start(shared: Shared, job: Job) {
    std::thread::spawn(move || {
        info!("the server asked to update {} -> {}", job.from, job.to);
        job.say(Step::Accepted, format!("downloading {}", job.to));
        match prepare(&shared, &job) {
            Ok(()) => {
                job.say(Step::Restarting, format!("restarting with {}", job.to));
                info!("installed {}; restarting", job.to);
                *shared.after.lock().expect("after lock") = Some(shared.me.clone());
                shared.stop.send_replace(true);
            }
            Err(why) => {
                warn!("not updating to {}: {why}", job.to);
                job.say(Step::Failed, why);
                shared.busy.store(false, Ordering::Release);
            }
        }
    });
}

fn prepare(shared: &Shared, job: &Job) -> Result<(), String> {
    let staged =
        crate::update::fetch_verified(&job.to, &shared.me, &mut std::io::sink()).map_err(text)?;
    accepts_config(staged.binary(), &shared.config, &job.to)?;
    let previous = previous_path(&shared.me);
    crate::service::copy_file(&shared.me, &previous, true).map_err(text)?;
    crate::service::copy_file(staged.binary(), &shared.me, true).map_err(text)?;
    let path = record_path(&shared.me);
    if let Err(e) = record::write(&path, &job.record(&shared.config), 0o600) {
        let _ = crate::service::copy_file(&previous, &shared.me, true);
        return Err(format!(
            "cannot write {}: {e}; nothing was changed",
            path.display()
        ));
    }
    Ok(())
}

fn accepts_config(new: &Path, config: &Path, version: &str) -> Result<(), String> {
    let out = std::process::Command::new(new)
        .arg("validate")
        .arg("-c")
        .arg(config)
        .output()
        .map_err(|e| format!("cannot run {version}: {e}; nothing was changed"))?;
    if out.status.success() {
        return Ok(());
    }
    let said =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let first = said
        .lines()
        .find(|l| l.trim_start().starts_with("error"))
        .unwrap_or_else(|| said.lines().next().unwrap_or(""))
        .trim()
        .to_string();
    Err(format!(
        "{version} does not accept {}: {first}; nothing was changed",
        config.display()
    ))
}

#[cfg(unix)]
pub fn restart(me: &Path) -> Result<u8, String> {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(me)
        .args(std::env::args_os().skip(1))
        .exec();
    Err(format!("cannot start {} again: {err}", me.display()))
}

#[cfg(not(unix))]
pub fn restart(me: &Path) -> Result<u8, String> {
    std::process::Command::new(me)
        .args(std::env::args_os().skip(1))
        .spawn()
        .map(|_| 0)
        .map_err(|e| format!("cannot start {} again: {e}", me.display()))
}
