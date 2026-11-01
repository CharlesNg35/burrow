use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use super::{AppConn, H2, Key, Pool};
use crate::http::Proto;

pub const APP_CONNECTIONS: usize = 8;

pub(crate) fn in_flight(proto: Proto) -> usize {
    match proto {
        Proto::H1 => APP_CONNECTIONS,
        Proto::H2 => usize::try_from(crate::http::h2::MAX_STREAMS).unwrap_or(APP_CONNECTIONS),
    }
}

struct Reserved {
    key: Key,
    client: Arc<str>,
    conn: H2,
    streams: usize,
    epoch: u64,
}

type Reservations = Arc<Mutex<Vec<Reserved>>>;

fn locked(r: &Reservations) -> MutexGuard<'_, Vec<Reserved>> {
    r.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct Ups {
    pub id: u64,
    slots: Arc<Semaphore>,
    reserved: Reservations,
    private: AtomicBool,
}

pub(crate) struct Slot {
    _permit: OwnedSemaphorePermit,
}

pub(crate) struct Stream {
    reserved: Reservations,
    pool: Arc<Pool>,
    id: u64,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

impl Ups {
    pub(crate) fn new(proto: Proto) -> Ups {
        Ups {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            slots: Arc::new(Semaphore::new(in_flight(proto))),
            reserved: Reservations::default(),
            private: AtomicBool::new(false),
        }
    }

    pub(crate) async fn slot(&self, deadline: Instant) -> Option<Slot> {
        let wait = self.slots.clone().acquire_owned();
        match tokio::time::timeout_at(deadline, wait).await {
            Ok(Ok(p)) => Some(Slot { _permit: p }),
            _ => None,
        }
    }

    pub(crate) fn has_private(&self) -> bool {
        self.private.load(Ordering::Acquire)
    }

    pub(crate) fn went_private(&self) {
        self.private.store(true, Ordering::Release);
    }

    pub(crate) fn take_h2(&self, pool: &Arc<Pool>, key: &Key) -> Option<(H2, Stream, Instant)> {
        let mut reserved = locked(&self.reserved);
        let held = reserved
            .iter_mut()
            .find(|r| r.key == *key && !r.conn.is_closed());
        let (conn, since) = match held {
            Some(r) => {
                r.streams += 1;
                (r.conn.clone(), Instant::now())
            }
            None => {
                let (conn, client, since) = pool.h2.take(key)?;
                reserved.push(Reserved {
                    key: key.clone(),
                    client,
                    conn: conn.clone(),
                    streams: 1,
                    epoch: pool.epoch(),
                });
                (conn, since)
            }
        };
        let stream = self.stream(pool, conn.id);
        Some((conn, stream, since))
    }

    pub(crate) fn reserve(
        &self,
        pool: &Arc<Pool>,
        (key, client): (Key, Arc<str>),
        conn: &H2,
        epoch: u64,
    ) -> Stream {
        locked(&self.reserved).push(Reserved {
            key,
            client,
            conn: conn.clone(),
            streams: 1,
            epoch,
        });
        self.stream(pool, conn.id)
    }

    fn stream(&self, pool: &Arc<Pool>, id: u64) -> Stream {
        Stream {
            reserved: self.reserved.clone(),
            pool: pool.clone(),
            id,
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let mut reserved = locked(&self.reserved);
        let Some(i) = reserved.iter().position(|r| r.conn.id == self.id) else {
            return;
        };
        let r = &mut reserved[i];
        r.streams = r.streams.saturating_sub(1);
        if r.streams > 0 {
            return;
        }
        let r = reserved.swap_remove(i);
        drop(reserved);
        if r.epoch == self.pool.epoch() {
            self.pool.h2.put(r.key, r.client, r.conn);
        }
    }
}
