use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tracing::{info, warn};

use super::Job;
use super::record::{self, Asked, Record};
use crate::install::Manifest;
use crate::service::helper;
use crate::wire::protocol::{LastUpdate, Outcome, Step};

pub(super) const WATCH_FOR: Duration = Duration::from_secs(30 * 60);

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
    let trigger = helper::trigger(m)?;
    (!super::by_hand::can_replace(&trigger)).then(|| {
        format!(
            "this service cannot write to {}",
            trigger.parent().unwrap_or(&trigger).display()
        )
    })
}

pub(super) fn asked_file(m: &Manifest) -> PathBuf {
    helper::own_dir(m).join("asked.json")
}

fn result_of(m: &Manifest) -> Option<Record> {
    record::read(&helper::result_file(m))
}

pub fn last(m: &Manifest) -> Option<LastUpdate> {
    last_from(&helper::result_file(m), &asked_file(m))
}

pub(super) fn last_from(result: &std::path::Path, asked: &std::path::Path) -> Option<LastUpdate> {
    let mut r: Record = record::read(result)?;
    r.id = record::read::<Asked>(asked)
        .filter(|a| answers(&r, a))
        .map_or(0, |a| a.id);
    r.last()
}

pub(super) fn answers(r: &Record, a: &Asked) -> bool {
    let after = match (r.at_ms, a.at_ms) {
        (0, _) | (_, 0) => r.at >= a.at,
        (r, a) => r >= a,
    };
    after && r.from == a.from
}

pub fn start(m: &Manifest, job: Job, busy: Arc<AtomicBool>) -> Result<(), String> {
    let asked = Asked {
        id: job.id,
        at: record::now(),
        from: job.from.clone(),
        by: String::new(),
        at_ms: record::now_ms(),
    };
    let file = asked_file(m);
    record::write(&file, &asked, 0o600)
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
        format!("{} downloads the latest release", helper::helper_name(m)),
    );
    let m = m.clone();
    std::thread::spawn(move || watch(&m, &asked, &job, &busy));
    Ok(())
}

fn watch(m: &Manifest, asked: &Asked, job: &Job, busy: &AtomicBool) {
    let until = Instant::now() + WATCH_FOR;
    while Instant::now() < until {
        std::thread::sleep(Duration::from_secs(2));
        let Some(r) = result_of(m).filter(|r| answers(r, asked)) else {
            continue;
        };
        match r.outcome() {
            Some(Outcome::Failed) => {
                warn!("not updating: {}", r.detail);
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
    let Some(waiting) = result_of(m) else {
        return;
    };
    if waiting.outcome() != Some(Outcome::Trying) || waiting.to == crate::base::build::version() {
        return;
    }
    let m = m.clone();
    std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(120);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_secs(1));
            if result_of(&m)
                .is_some_and(|r| r.at != waiting.at || r.outcome() != Some(Outcome::Trying))
            {
                again.send_modify(|n| *n += 1);
                return;
            }
        }
    });
}
