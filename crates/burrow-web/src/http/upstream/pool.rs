use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::Way;

mod back;
mod busy;
mod conns;
mod store;
mod visitor;

pub(crate) use back::{Back, Home};
pub(crate) use busy::{Cut, CutOff, Cutter, Ticket};
pub(crate) use conns::{AppConn, Counted, H1, H2};
pub(crate) use store::Store;
pub(crate) use visitor::{Slot, Stream, Ups};

pub const IDLE: Duration = Duration::from_secs(60);
pub const SWEEP: Duration = Duration::from_secs(5);
pub const REUSE_WAIT: Duration = Duration::from_secs(1);
pub const EVICT_WAIT: Duration = Duration::from_secs(2);
pub const EVICT_TRIES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Bound {
    Shared,
    Visitor(SocketAddr, SocketAddr),
    Private(u64, IpAddr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub service: Arc<str>,
    pub way: u64,
    pub site: u64,
    pub bound: Bound,
}

impl Key {
    pub(crate) fn new(service: &Arc<str>, way: &Way, site: u64) -> Key {
        Key {
            service: service.clone(),
            way: way.id,
            site,
            bound: way
                .bound
                .map_or(Bound::Shared, |(a, b)| Bound::Visitor(a, b)),
        }
    }

    pub(crate) fn answered(&self, via: Option<u64>) -> Key {
        match via {
            Some(way) if way != self.way && !matches!(self.bound, Bound::Private(..)) => Key {
                way,
                ..self.clone()
            },
            _ => self.clone(),
        }
    }

    pub(crate) fn private(&self, conn: u64, visitor: IpAddr) -> Key {
        Key {
            way: 0,
            bound: Bound::Private(conn, visitor),
            ..self.clone()
        }
    }
}

#[derive(Default)]
pub(crate) struct Pool {
    pub h1: Store<H1>,
    pub h2: Store<H2>,
    pub busy: busy::Busy,
    epoch: AtomicU64,
    closing: std::sync::Mutex<Vec<crate::Ending>>,
}

impl Pool {
    pub(crate) fn new() -> Arc<Pool> {
        Arc::new(Pool::default())
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    pub(crate) fn sweep(&self, crowded: impl Fn(&str) -> bool) {
        let mut services = self.h1.services();
        services.extend(self.h2.services());
        services.sort_unstable();
        services.dedup();
        let pressed: Vec<Arc<str>> = services.into_iter().filter(|s| crowded(s)).collect();
        let limit = |service: &str| {
            if pressed.iter().any(|p| &**p == service) {
                SWEEP
            } else {
                IDLE
            }
        };
        self.h1.sweep(&limit);
        self.h2.sweep(&limit);
    }

    pub(crate) fn evict(&self, service: &str, client: &str) -> Vec<crate::Ending> {
        let mut closing = self
            .closing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        closing.retain(|e| !e.is_ended());
        closing.extend(self.h1.evict(service, client));
        closing.extend(self.h2.evict(service, client));
        closing.clone()
    }

    pub(crate) fn reloaded(&self, current: impl Fn(&Key) -> bool) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.h1.retain(&current);
        self.h2.retain(&current);
    }

    pub(crate) fn clear(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.h1.retain(&|_: &Key| false);
        self.h2.retain(&|_: &Key| false);
    }
}

#[cfg(test)]
mod tests;
