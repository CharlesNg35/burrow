use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Waker};

use tokio::sync::Notify;
use tokio::task::AbortHandle;

use super::Pool;
use crate::{Ask, Visitor};

struct Held {
    service: Arc<str>,
    from: Visitor,
    ask: Ask,
    cutter: Cutter,
    epoch: u64,
}

pub(crate) enum Cutter {
    Connection(AbortHandle),
    Stream(Arc<Cut>),
}

impl Cutter {
    fn cut(&self) {
        match self {
            Cutter::Connection(task) => task.abort(),
            Cutter::Stream(cut) => cut.trigger(),
        }
    }
}

#[derive(Debug)]
pub struct CutOff;

impl std::fmt::Display for CutOff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the visitor is no longer allowed to reach this service")
    }
}

impl std::error::Error for CutOff {}

#[derive(Default)]
pub(crate) struct Cut {
    done: AtomicBool,
    notify: Notify,
    waker: Mutex<Option<Waker>>,
}

impl Cut {
    fn trigger(&self) {
        self.done.store(true, Ordering::Release);
        self.notify.notify_waiters();
        let waker = self
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(w) = waker {
            w.wake();
        }
    }

    pub(crate) fn is_cut(&self, cx: &Context<'_>) -> bool {
        if self.done.load(Ordering::Acquire) {
            return true;
        }
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = Some(cx.waker().clone());
        self.done.load(Ordering::Acquire)
    }

    pub(crate) async fn wait(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.done.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

const SHARDS: usize = 16;

#[derive(Default)]
pub(crate) struct Busy {
    next: AtomicU64,
    shards: [Mutex<HashMap<u64, Held>>; SHARDS],
}

pub(crate) struct Ticket {
    pool: Arc<Pool>,
    held: Option<Held>,
    id: Option<u64>,
    stream: Option<Arc<Cut>>,
}

impl Ticket {
    pub(crate) fn stream_cut(&self) -> Option<Arc<Cut>> {
        self.stream.clone()
    }

    pub(crate) fn waiting(&mut self) {
        let Some(held) = self.held.take() else {
            return;
        };
        let id = self.pool.busy.next.fetch_add(1, Ordering::Relaxed);
        self.pool.busy.shard(id).insert(id, held);
        self.id = Some(id);
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.pool.busy.shard(id).remove(&id);
        }
    }
}

type Keeps<'a> = &'a dyn Fn(&str, &Visitor, Ask) -> bool;

impl Busy {
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        (0..SHARDS).map(|i| self.at(i).len()).sum()
    }

    fn at(&self, i: usize) -> MutexGuard<'_, HashMap<u64, Held>> {
        self.shards[i]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn shard(&self, id: u64) -> MutexGuard<'_, HashMap<u64, Held>> {
        self.at(usize::try_from(id).unwrap_or(0) % SHARDS)
    }

    fn check(&self, epoch: u64, which: &dyn Fn(u64) -> bool, keeps: Keeps<'_>) {
        for i in 0..SHARDS {
            let held: Vec<(u64, Arc<str>, Visitor, Ask)> = self
                .at(i)
                .iter()
                .filter(|(_, h)| which(h.epoch))
                .map(|(id, h)| (*id, h.service.clone(), h.from.clone(), h.ask))
                .collect();
            let gone: Vec<u64> = held
                .iter()
                .filter(|(_, service, from, ask)| !keeps(service, from, *ask))
                .map(|(id, ..)| *id)
                .collect();
            let mut all = self.at(i);
            for (id, ..) in &held {
                let Some(h) = all.get_mut(id) else {
                    continue;
                };
                h.epoch = epoch;
                if gone.contains(id) {
                    tracing::debug!(peer = %h.from.addr, "web: service \"{}\" no longer takes this visitor, closing its answer", h.service);
                    h.cutter.cut();
                }
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
        cutter: Cutter,
        epoch: u64,
    ) -> Ticket {
        let stream = match &cutter {
            Cutter::Stream(cut) => Some(cut.clone()),
            Cutter::Connection(_) => None,
        };
        let held = Held {
            service: service.clone(),
            from,
            ask,
            cutter,
            epoch,
        };
        Ticket {
            pool: self.clone(),
            held: Some(held),
            id: None,
            stream,
        }
    }
}
