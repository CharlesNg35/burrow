use std::future::Future;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use burrow_syntax::share::{Share, Shares};
pub use burrow_syntax::share::{source_key, subnet_key};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

pub const WAITING_PER_SLOT: usize = 4;
pub const STALLED_AFTER: Duration = Duration::from_secs(1);

#[derive(Clone)]
struct Pools {
    slots: Arc<Semaphore>,
    room: Arc<Semaphore>,
    arrived: Arc<AtomicUsize>,
    total: usize,
}

struct Arrived(Arc<AtomicUsize>);

impl Arrived {
    fn new(count: &Arc<AtomicUsize>) -> Arrived {
        count.fetch_add(1, Ordering::Relaxed);
        Arrived(count.clone())
    }
}

impl Drop for Arrived {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Pools {
    fn new(total: usize) -> Pools {
        Pools {
            slots: Arc::new(Semaphore::new(total)),
            room: Arc::new(Semaphore::new(room_for(total))),
            arrived: Arc::default(),
            total,
        }
    }

    fn handshaking(&self) -> usize {
        self.total.saturating_sub(self.slots.available_permits())
    }

    fn waiting(&self) -> usize {
        let admitted = self.arrived.load(Ordering::Relaxed);
        admitted.saturating_sub(self.handshaking())
    }
}

fn room_for(total: usize) -> usize {
    total.saturating_mul(WAITING_PER_SLOT + 1)
}

pub struct Permits {
    pools: RwLock<Pools>,
    waiting: Shares<IpAddr>,
    stalled: Shares<IpAddr>,
    refused: AtomicU64,
}

pub struct Room {
    room: OwnedSemaphorePermit,
    pools: Pools,
}

pub struct Entry {
    room: (OwnedSemaphorePermit, Arrived),
    pools: Pools,
    since: Instant,
}

pub struct Permit {
    _slot: OwnedSemaphorePermit,
    _room: (OwnedSemaphorePermit, Arrived),
    pools: Pools,
    key: Option<IpAddr>,
    since: Instant,
    stalled: Option<Share<IpAddr>>,
    waited: bool,
}

impl Room {
    pub fn arrived(self) -> Entry {
        let arrived = Arrived::new(&self.pools.arrived);
        Entry {
            room: (self.room, arrived),
            pools: self.pools,
            since: Instant::now(),
        }
    }
}

impl Entry {
    pub fn since(&self) -> Instant {
        self.since
    }
}

impl Permit {
    pub fn waited(&self) -> bool {
        self.waited
    }

    pub fn hello_seen(&mut self) {
        self.since = Instant::now();
    }
}

impl Permits {
    pub fn new(total: usize) -> Permits {
        Permits {
            pools: RwLock::new(Pools::new(total)),
            waiting: Shares::default(),
            stalled: Shares::default(),
            refused: AtomicU64::new(0),
        }
    }

    fn pools(&self) -> Pools {
        self.pools.read().expect("pools lock").clone()
    }

    pub fn resize(&self, total: usize) {
        let mut pools = self.pools.write().expect("pools lock");
        if pools.total != total {
            pools.room.close();
            *pools = Pools::new(total);
        }
    }

    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    pub fn count_refused(&self) {
        self.refused.fetch_add(1, Ordering::Relaxed);
    }

    pub fn admitted(&self) -> usize {
        self.pools().arrived.load(Ordering::Relaxed)
    }

    pub async fn enter(&self) -> Room {
        loop {
            let pools = self.pools();
            if let Ok(room) = pools.room.clone().acquire_owned().await {
                return Room { room, pools };
            }
        }
    }

    pub async fn take(&self, entry: Entry, ip: IpAddr, deadline: Instant) -> Option<Permit> {
        let Entry { room, pools, since } = entry;
        let key = source_key(ip);
        let permit = |slot, waited| Permit {
            _slot: slot,
            _room: room,
            pools: pools.clone(),
            key,
            since,
            stalled: None,
            waited,
        };
        if let Ok(slot) = pools.slots.clone().try_acquire_owned() {
            return Some(permit(slot, false));
        }
        let _waiting = match key {
            Some(key) => {
                let total = pools.total.saturating_mul(WAITING_PER_SLOT);
                Some(self.waiting.take(key, total, pools.waiting())?)
            }
            None => None,
        };
        let slot = tokio::time::timeout_at(deadline, pools.slots.clone().acquire_owned()).await;
        Some(permit(slot.ok()?.ok()?, true))
    }

    fn stall(&self, permit: &mut Permit) -> bool {
        let Some(key) = permit.key else {
            return true;
        };
        if permit.stalled.is_some() {
            return true;
        }
        let pools = &permit.pools;
        permit.stalled = self.stalled.take(key, pools.total, pools.handshaking());
        permit.stalled.is_some()
    }

    pub async fn hold<F: Future>(&self, permit: &mut Permit, work: F) -> Option<F::Output> {
        let mut work = std::pin::pin!(work);
        if permit.key.is_some() && permit.stalled.is_none() {
            tokio::select! {
                biased;
                done = &mut work => return Some(done),
                _ = tokio::time::sleep_until(permit.since + STALLED_AFTER) => {}
            }
            if !self.stall(permit) {
                return None;
            }
        }
        Some(work.await)
    }
}

#[cfg(test)]
mod tests;
