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
    epoch: u64,
}

#[derive(Default)]
pub(crate) struct Busy {
    next: AtomicU64,
    all: Mutex<HashMap<u64, Held>>,
}

pub(crate) struct Ticket {
    pool: Arc<Pool>,
    held: Option<Held>,
    id: Option<u64>,
}

impl Ticket {
    pub(crate) fn waiting(&mut self) {
        let Some(held) = self.held.take() else {
            return;
        };
        let id = self.pool.busy.next.fetch_add(1, Ordering::Relaxed);
        self.pool.busy.locked().insert(id, held);
        self.id = Some(id);
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.pool.busy.locked().remove(&id);
        }
    }
}

type Keeps<'a> = &'a dyn Fn(&str, &Visitor, Ask) -> bool;

impl Busy {
    fn locked(&self) -> MutexGuard<'_, HashMap<u64, Held>> {
        self.all.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn check(&self, epoch: u64, which: &dyn Fn(u64) -> bool, keeps: Keeps<'_>) {
        let held: Vec<(u64, Arc<str>, Visitor, Ask)> = self
            .locked()
            .iter()
            .filter(|(_, h)| which(h.epoch))
            .map(|(id, h)| (*id, h.service.clone(), h.from.clone(), h.ask))
            .collect();
        let gone: Vec<u64> = held
            .iter()
            .filter(|(_, service, from, ask)| !keeps(service, from, *ask))
            .map(|(id, ..)| *id)
            .collect();
        let mut all = self.locked();
        for (id, ..) in &held {
            let Some(h) = all.get_mut(id) else {
                continue;
            };
            h.epoch = epoch;
            if gone.contains(id) {
                tracing::debug!(peer = %h.from.addr, "web: service \"{}\" no longer takes this visitor, closing its answer", h.service);
                h.task.abort();
            }
        }
    }

    pub(crate) fn cut(&self, epoch: u64, keeps: Keeps<'_>) {
        self.check(epoch, &|_| true, keeps);
    }

    pub(crate) fn recheck(&self, epoch: u64, keeps: Keeps<'_>) {
        self.check(epoch, &|e| e != epoch, keeps);
    }
}

impl Pool {
    pub(crate) fn ticket(
        self: &Arc<Self>,
        service: &Arc<str>,
        from: Visitor,
        ask: Ask,
        task: AbortHandle,
    ) -> Ticket {
        let held = Held {
            service: service.clone(),
            from,
            ask,
            task,
            epoch: self.epoch(),
        };
        Ticket {
            pool: self.clone(),
            held: Some(held),
            id: None,
        }
    }
}
