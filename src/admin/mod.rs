//! The server side of `burrow admin`: a `Hub` that seats sessions, queues plans and owns the admin core.
//! `core` holds the logic, `transport` runs a session, `adapters` connect the core to files and the running server.

pub mod adapters;
pub mod core;
pub mod hooks;
mod kept;
pub mod latest;
pub mod live;
pub mod local;
mod notes;
pub mod own_update;
pub mod transport;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{Semaphore, mpsc};

use crate::app::reload::Nudge;

pub const MAX_SESSIONS: usize = 64;
pub const MAX_SESSIONS_PER_VISITOR: usize = 32;
pub const MAX_INFLIGHT: usize = 8;
pub const MAX_QUEUED_PLANS: usize = 4;

pub struct Hub {
    pub core: Arc<core::Core>,
    sessions: Arc<Semaphore>,
    per_visitor: Mutex<HashMap<String, usize>>,
    plan_lock: tokio::sync::Mutex<()>,
    queued: AtomicUsize,
    nudge: mpsc::Sender<Nudge>,
    last_write: Mutex<Option<(String, String)>>,
}

pub struct Seat {
    hub: Arc<Hub>,
    visitor: String,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl Drop for Seat {
    fn drop(&mut self) {
        let mut m = self.hub.per_visitor.lock().expect("per visitor lock");
        if let Some(n) = m.get_mut(&self.visitor) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.visitor);
            }
        }
    }
}

pub struct Queued<'a> {
    hub: &'a Hub,
}

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        self.hub.queued.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Hub {
    pub fn new(core: core::Core, nudge: mpsc::Sender<Nudge>) -> Arc<Hub> {
        Arc::new(Hub {
            core: Arc::new(core),
            sessions: Arc::new(Semaphore::new(MAX_SESSIONS)),
            per_visitor: Mutex::new(HashMap::new()),
            plan_lock: tokio::sync::Mutex::new(()),
            queued: AtomicUsize::new(0),
            nudge,
            last_write: Mutex::new(None),
        })
    }

    pub fn seat(self: &Arc<Self>, visitor: &str) -> Option<Seat> {
        let permit = self.sessions.clone().try_acquire_owned().ok()?;
        let mut m = self.per_visitor.lock().expect("per visitor lock");
        let n = m.entry(visitor.to_string()).or_default();
        if *n >= MAX_SESSIONS_PER_VISITOR {
            return None;
        }
        *n += 1;
        Some(Seat {
            hub: self.clone(),
            visitor: visitor.to_string(),
            _permit: permit,
        })
    }

    pub fn queue(&self) -> Option<Queued<'_>> {
        let before = self.queued.fetch_add(1, Ordering::AcqRel);
        let q = Queued { hub: self };
        (before < MAX_QUEUED_PLANS).then_some(q)
    }

    pub async fn plan_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.plan_lock.lock().await
    }

    pub fn plan_lock_now(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.plan_lock.try_lock().ok()
    }

    pub fn nudger(&self) -> mpsc::Sender<Nudge> {
        self.nudge.clone()
    }

    pub fn wrote(&self, rev: &str, visitor: &str) {
        *self.last_write.lock().expect("last write lock") =
            Some((rev.to_string(), visitor.to_string()));
    }

    pub fn written_by(&self, rev: &str) -> Option<String> {
        self.last_write
            .lock()
            .expect("last write lock")
            .as_ref()
            .filter(|(r, _)| r == rev)
            .map(|(_, v)| v.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::core::tests::{core_for, world};

    #[test]
    fn one_visitor_holds_32_admin_sessions_and_another_still_gets_one() {
        let (tx, _rx) = mpsc::channel(1);
        let hub = Hub::new(core_for(&world().text), tx);
        let held: Vec<Seat> = (0..32)
            .map(|n| hub.seat("ops").unwrap_or_else(|| panic!("seat {n}")))
            .collect();
        assert!(hub.seat("ops").is_none());
        let other = hub.seat("bob");
        assert!(other.is_some());
        drop(held);
        assert!(hub.seat("ops").is_some());
    }
}
