pub mod adapters;
pub mod core;
pub mod hooks;
pub mod transport;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{Semaphore, mpsc};

use crate::reload::Nudge;

pub const MAX_SESSIONS: usize = 8;
pub const MAX_SESSIONS_PER_VISITOR: usize = 2;
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

pub struct Opened {
    pub visitor: String,
    pub secret: crate::wire::secret::Secret,
    pub peer: SocketAddr,
    pub via_websocket: bool,
}
