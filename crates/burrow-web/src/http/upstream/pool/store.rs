use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use super::{AppConn, Key};
use crate::Ending;

const READY_SCAN: usize = 8;

struct Parked<C> {
    conn: C,
    client: Arc<str>,
    since: Instant,
}

pub(crate) struct Store<C> {
    idle: Mutex<HashMap<Key, Vec<Parked<C>>>>,
}

impl<C> Default for Store<C> {
    fn default() -> Self {
        Store {
            idle: Mutex::default(),
        }
    }
}

impl<C: AppConn> Store<C> {
    fn locked(&self) -> MutexGuard<'_, HashMap<Key, Vec<Parked<C>>>> {
        self.idle.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn take(&self, key: &Key) -> Option<(C, Arc<str>)> {
        let mut idle = self.locked();
        let list = idle.get_mut(key)?;
        while list.last().is_some_and(|p| p.conn.is_closed()) {
            list.pop();
        }
        let ready = list
            .iter()
            .rev()
            .take(READY_SCAN)
            .position(|p| p.conn.is_ready())
            .map(|from_end| list.len() - 1 - from_end);
        let mut found = ready.map(|i| {
            let p = list.remove(i);
            (p.conn, p.client)
        });
        while found.is_none()
            && let Some(p) = list.pop()
        {
            if !p.conn.is_closed() {
                found = Some((p.conn, p.client));
            }
        }
        if list.is_empty() {
            idle.remove(key);
        }
        found
    }

    pub(crate) fn put(&self, key: Key, client: Arc<str>, conn: C) {
        if conn.is_closed() {
            return;
        }
        let parked = Parked {
            conn,
            client,
            since: Instant::now(),
        };
        self.locked().entry(key).or_default().push(parked);
    }

    pub(crate) fn services(&self) -> Vec<Arc<str>> {
        self.locked().keys().map(|k| k.service.clone()).collect()
    }

    pub(crate) fn sweep(&self, limit: &impl Fn(&str) -> Duration) {
        let now = Instant::now();
        let mut gone = Vec::new();
        let mut idle = self.locked();
        idle.retain(|key, list| {
            let most = limit(&key.service);
            let (keep, old): (Vec<_>, Vec<_>) = std::mem::take(list)
                .into_iter()
                .partition(|p| !p.conn.is_closed() && now.duration_since(p.since) < most);
            *list = keep;
            gone.extend(old);
            !list.is_empty()
        });
        drop(idle);
        drop(gone);
    }

    pub(crate) fn evict(&self, service: &str, client: &str) -> Vec<Ending> {
        let mut gone = Vec::new();
        let mut idle = self.locked();
        idle.retain(|key, list| {
            if &*key.service == service {
                gone.append(list);
            } else {
                let (keep, out): (Vec<_>, Vec<_>) = std::mem::take(list)
                    .into_iter()
                    .partition(|p| &*p.client != client);
                *list = keep;
                gone.extend(out);
            }
            !list.is_empty()
        });
        drop(idle);
        gone.into_iter()
            .filter_map(|p| {
                let ending = p.conn.ending();
                p.conn.close();
                ending
            })
            .collect()
    }

    pub(crate) fn retain(&self, keep: &impl Fn(&Key) -> bool) {
        let mut idle = self.locked();
        let mut gone = Vec::new();
        idle.retain(|key, list| {
            if keep(key) {
                return true;
            }
            gone.append(list);
            false
        });
        drop(idle);
        for p in gone {
            p.conn.close();
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.locked().values().map(Vec::len).sum()
    }
}
