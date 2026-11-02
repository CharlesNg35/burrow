use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tracing::{info, warn};

use super::Job;
use super::record::{self, Record, Request};
use crate::install::Manifest;
use crate::service::helper;
use crate::wire::protocol::{Outcome, Step};

const WATCH_FOR: Duration = Duration::from_secs(30 * 60);

pub fn blocked(m: &Manifest) -> Option<String> {
    if !m.remote_updates || helper::helper().is_none() {
        return Some(format!(
            "the service needs installing again to allow it: {}",
            crate::base::failure::admin(&format!(
                "service install -c {}",
                crate::base::failure::path_arg(&m.source_config)
            ))
        ));
    }
    let dir = helper::dir(m);
    (!super::by_hand::can_replace(&dir.join("x")))
        .then(|| format!("this service cannot write to {}", dir.display()))
}

fn place(dir: &Path, file: &Path, request: &Request) -> std::io::Result<()> {
    let tmp = dir.join(".request.tmp");
    record::write(&tmp, request, 0o600)?;
    std::fs::rename(&tmp, file)
}

pub fn start(m: &Manifest, job: Job, busy: Arc<AtomicBool>) -> Result<(), String> {
    let file = helper::request_file(m).ok_or("this system has no updater")?;
    let request = Request {
        id: job.id,
        version: job.to.clone(),
    };
    place(&helper::dir(m), &file, &request)
        .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    helper::fire(m)?;
    info!(
        "the server asked to update {} -> {}; {} does it",
        job.from,
        job.to,
        helper::helper_name(m)
    );
    job.say(
        Step::Accepted,
        format!("{} downloads {}", helper::helper_name(m), job.to),
    );
    let result = helper::result_file(m);
    std::thread::spawn(move || watch(&result, &job, &busy));
    Ok(())
}

fn watch(result: &Path, job: &Job, busy: &AtomicBool) {
    let until = Instant::now() + WATCH_FOR;
    while Instant::now() < until {
        std::thread::sleep(Duration::from_secs(2));
        let Some(r) = record::read::<Record>(result).filter(|r| r.id == job.id) else {
            continue;
        };
        match r.outcome() {
            Some(Outcome::Failed) => {
                warn!("not updating to {}: {}", job.to, r.detail);
                job.say(Step::Failed, r.detail);
                break;
            }
            Some(Outcome::Trying) | None => {}
            Some(_) => break,
        }
    }
    busy.store(false, Ordering::Release);
}

pub fn await_result(m: &Manifest, again: Arc<tokio::sync::watch::Sender<u64>>) {
    let result = helper::result_file(m);
    let Some(waiting) = record::read::<Record>(&result) else {
        return;
    };
    if waiting.outcome() != Some(Outcome::Trying) || waiting.to == crate::base::build::version() {
        return;
    }
    std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(120);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_secs(1));
            let now = record::read::<Record>(&result);
            if now
                .as_ref()
                .is_some_and(|r| r.id != waiting.id || r.outcome() != Some(Outcome::Trying))
            {
                again.send_modify(|n| *n += 1);
                return;
            }
        }
    });
}
