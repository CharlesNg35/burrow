pub mod basic;
mod cache;
pub mod forward;
pub mod hash;
pub mod limit;
pub mod oidc;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;
use tracing::warn;

pub use hash::{HashProblem, check_hash, hash_password, verify_hash};
pub use limit::{Failures, Limit, retry_after};

use crate::BasicUser;
use cache::Cache;
use hash::Stored;

pub const SLOTS: usize = 2;
pub const SLOT_WAIT: Duration = Duration::from_secs(1);

#[derive(Clone, Default)]
pub(crate) struct Users(HashMap<String, (String, Option<Stored>)>);

impl Users {
    pub(crate) fn build(service: &str, list: &[BasicUser]) -> Users {
        let map = list
            .iter()
            .map(|u| {
                let stored = hash::stored(&u.hash)
                    .inspect_err(|e| {
                        warn!(
                            "web: service \"{service}\": basic_auth: the hash for \"{}\" {e}; that user cannot log in",
                            u.user
                        );
                    })
                    .ok();
                (u.user.clone(), (u.hash.clone(), stored))
            })
            .collect();
        Users(map)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Granted,
    Wrong,
    Wait(Duration),
    Busy,
}

pub(crate) struct Access {
    slots: Arc<Semaphore>,
    cache: Cache,
    pub(crate) failures: Failures,
    #[cfg(test)]
    work: Arc<tests::Work>,
}

impl Default for Access {
    fn default() -> Access {
        Access {
            slots: Arc::new(Semaphore::new(SLOTS)),
            cache: Cache::new(),
            failures: Failures::default(),
            #[cfg(test)]
            work: Arc::default(),
        }
    }
}

impl Access {
    pub(crate) async fn check(&self, users: &Users, header: Option<&[u8]>, ip: IpAddr) -> Verdict {
        if let Some(wait) = self.failures.blocked(ip) {
            return Verdict::Wait(wait);
        }
        let Some(creds) = header.and_then(basic::parse) else {
            return Verdict::Wrong;
        };
        let known = users.0.get(&creds.user);
        let tag = known.map(|(h, _)| self.cache.tag(&creds.user, &creds.password, h));
        if tag.is_some_and(|t| self.cache.hit(&t, Instant::now())) {
            return Verdict::Granted;
        }
        let Ok(Ok(slot)) =
            tokio::time::timeout(SLOT_WAIT, self.slots.clone().acquire_owned()).await
        else {
            return Verdict::Busy;
        };
        let stored = match known {
            Some((_, Some(s))) => Some(s.clone()),
            _ => None,
        };
        let password = creds.password;
        #[cfg(test)]
        let work = self.work.clone();
        let good = tokio::task::spawn_blocking(move || {
            let _slot = slot;
            #[cfg(test)]
            let _counted = work.enter(stored.is_none());
            match &stored {
                Some(s) => hash::verify(&password, s),
                None => {
                    hash::verify(&password, hash::dummy());
                    false
                }
            }
        })
        .await
        .unwrap_or(false);
        match (good, tag) {
            (true, Some(t)) => {
                self.cache.keep(t, Instant::now());
                Verdict::Granted
            }
            _ => self
                .failures
                .failed(ip)
                .map_or(Verdict::Wrong, Verdict::Wait),
        }
    }

    pub(crate) fn sweep(&self) {
        self.cache.sweep(Instant::now());
        self.failures.sweep();
    }
}

#[cfg(test)]
mod tests;
