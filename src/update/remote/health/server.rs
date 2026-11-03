use std::cell::OnceCell;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::{Health, own_status};
use crate::install::Manifest;
use crate::metrics::Status;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    pub quiet: BTreeSet<String>,
    pub web_waiting: bool,
    pub clients: BTreeSet<String>,
}

impl Seen {
    pub fn of(s: &Status) -> Seen {
        Seen {
            quiet: s
                .services
                .iter()
                .filter(|v| v.not_listening.is_some())
                .map(|v| v.name.clone())
                .collect(),
            web_waiting: s.web_waiting.is_some(),
            clients: connected(s),
        }
    }
}

fn connected(s: &Status) -> BTreeSet<String> {
    s.clients
        .iter()
        .filter(|c| c.connected && !c.local)
        .map(|c| c.name.clone())
        .collect()
}

pub fn judge(s: &Status, version: &str, before: &Seen) -> Result<(), String> {
    if s.version != version {
        return Err(format!("it reports {}, not {version}", s.version));
    }
    if let Some((name, why)) = s
        .services
        .iter()
        .filter(|v| !before.quiet.contains(&v.name))
        .find_map(|v| v.not_listening.as_ref().map(|w| (&v.name, w)))
    {
        return Err(format!(
            "{version} does not listen for service \"{name}\": {why}"
        ));
    }
    match &s.web_waiting {
        Some(w) if !before.web_waiting => Err(format!(
            "{version} does not listen on {} {}: {}",
            w.key, w.addr, w.reason
        )),
        _ => Ok(()),
    }
}

pub fn back(before: &Seen, now: &Status) -> String {
    if before.clients.is_empty() {
        return String::new();
    }
    let now = connected(now);
    let back = before.clients.intersection(&now).count();
    let all = before.clients.len();
    match back == all {
        true => format!("{back} of {all} clients connected again"),
        false => format!(
            "{back} of {all} clients connected again so far; the others reconnect on their own"
        ),
    }
}

#[derive(Default)]
pub struct Serving {
    before: OnceCell<Seen>,
}

impl Serving {
    fn seen(&self) -> Seen {
        self.before.get().cloned().unwrap_or_default()
    }
}

impl Health for Serving {
    fn before(&self, m: &Manifest) {
        let _ = self
            .before
            .set(own_status(m).as_ref().map(Seen::of).unwrap_or_default());
    }

    fn probe(&self, m: &Manifest, version: &str) -> Result<(), String> {
        if crate::service::main_pid(m).is_none() {
            return Err(format!("{version} is not running"));
        }
        let s = own_status(m)
            .ok_or_else(|| format!("{version} did not answer on its local status endpoint"))?;
        judge(&s, version, &self.seen())
    }

    fn note(&self, m: &Manifest, _version: &str) -> String {
        let before = self.seen();
        let until = Instant::now() + (super::super::window() / 3).min(Duration::from_secs(20));
        loop {
            let now = own_status(m).unwrap_or_default();
            let done = connected(&now).is_superset(&before.clients);
            if done || Instant::now() >= until {
                return back(&before, &now);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}
