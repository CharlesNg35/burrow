//! How the root updater tells that a service runs well on its new release: one implementation per role.
//! The updater polls `probe` until it passes or the window ends, then rolls back; `note` adds what the result should say.

mod client;
mod server;

use std::time::Instant;

use crate::install::{Manifest, Role};

pub use client::Connected;
pub use server::Serving;

pub trait Health {
    fn before(&self, m: &Manifest);
    fn probe(&self, m: &Manifest, version: &str) -> Result<(), String>;
    fn note(&self, m: &Manifest, version: &str) -> String;
}

pub fn of(role: Role) -> Option<Box<dyn Health>> {
    match role {
        Role::Client => Some(Box::new(Connected)),
        Role::Server => Some(Box::new(Serving::default())),
        Role::Visitor => None,
    }
}

pub fn wait(
    health: &dyn Health,
    m: &Manifest,
    version: &str,
    window: std::time::Duration,
) -> Result<(), String> {
    let until = Instant::now() + window;
    let mut last = format!("{version} did not start");
    while Instant::now() < until {
        match health.probe(m, version) {
            Ok(()) => return Ok(()),
            Err(why) => last = why,
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err(format!("{last} within {} s", window.as_secs()))
}

pub(super) fn own_status(m: &Manifest) -> Option<crate::metrics::Status> {
    let pid = crate::service::main_pid(m)?;
    let config = crate::base::fs::canonical_or_same(&m.config);
    let scan = crate::status::query::scan(&crate::instance::default_dir(), Some(&config));
    scan.found
        .into_iter()
        .find(|f| f.entry.pid == pid)
        .map(|f| f.status)
}

#[cfg(test)]
mod tests;
