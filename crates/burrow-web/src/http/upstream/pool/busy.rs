use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::task::AbortHandle;

use super::Pool;
use crate::{Ask, Visitor};

struct Held {
    service: Arc<str>,
    from: Visitor,
    ask: Ask,
    task: AbortHandle,
}

#[derive(Default)]
pub(crate) struct Busy {
    next: AtomicU64,
    all: Mutex<HashMap<u64, Held>>,
}

pub(crate) struct Ticket {
    pool: Arc<Pool>,
    id: u64,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.pool.busy.locked().remove(&self.id);
    }
}

impl Busy {
    fn locked(&self) -> MutexGuard<'_, HashMap<u64, Held>> {
        self.all.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn cut(&self, keeps: impl Fn(&str, &Visitor, Ask) -> bool) {
        let held: Vec<(u64, Arc<str>, Visitor, Ask)> = self
            .locked()
            .iter()
            .map(|(id, h)| (*id, h.service.clone(), h.from.clone(), h.ask))
            .collect();
        let gone: Vec<u64> = held
            .into_iter()
            .filter(|(_, service, from, ask)| !keeps(service, from, *ask))
            .map(|(id, ..)| id)
            .collect();
        let all = self.locked();
        for id in gone {
            if let Some(h) = all.get(&id) {
                tracing::debug!(peer = %h.from.addr, "web: service \"{}\" no longer takes this visitor, closing its answer", h.service);
                h.task.abort();
            }
        }
    }
}

impl Pool {
    pub(crate) fn enter(
        self: &Arc<Self>,
        service: &Arc<str>,
        from: &Visitor,
        ask: Ask,
        task: AbortHandle,
    ) -> Ticket {
        let id = self.busy.next.fetch_add(1, Ordering::Relaxed);
        let held = Held {
            service: service.clone(),
            from: from.clone(),
            ask,
            task,
        };
        self.busy.locked().insert(id, held);
        Ticket {
            pool: self.clone(),
            id,
        }
    }
}
