use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct PeerTable<K> {
    by_key: HashMap<K, (u32, Instant)>,
    by_id: HashMap<u32, K>,
    next: u32,
}

impl<K> Default for PeerTable<K> {
    fn default() -> Self {
        Self {
            by_key: HashMap::new(),
            by_id: HashMap::new(),
            next: 0,
        }
    }
}

impl<K: Copy + Eq + Hash> PeerTable<K> {
    pub fn id_for(&mut self, key: K, most: impl FnOnce() -> usize) -> Option<u32> {
        if let Some((id, seen)) = self.by_key.get_mut(&key) {
            *seen = Instant::now();
            return Some(*id);
        }
        if self.by_key.len() >= most() {
            return None;
        }
        self.next = self.next.wrapping_add(1);
        while self.by_id.contains_key(&self.next) {
            self.next = self.next.wrapping_add(1);
        }
        let id = self.next;
        self.by_key.insert(key, (id, Instant::now()));
        self.by_id.insert(id, key);
        Some(id)
    }

    pub fn touch(&mut self, id: u32) -> Option<K> {
        let key = *self.by_id.get(&id)?;
        if let Some((_, seen)) = self.by_key.get_mut(&key) {
            *seen = Instant::now();
        }
        Some(key)
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    pub fn expire(&mut self, timeout: Duration) {
        self.retain_where(|_, seen| seen.elapsed() < timeout);
    }

    pub fn retain(&mut self, keep: impl Fn(K) -> bool) {
        self.retain_where(|key, _| keep(key));
    }

    fn retain_where(&mut self, keep: impl Fn(K, Instant) -> bool) {
        let by_id = &mut self.by_id;
        self.by_key.retain(|key, (id, seen)| {
            let kept = keep(*key, *seen);
            if !kept {
                by_id.remove(id);
            }
            kept
        });
    }
}

pub struct AbortOnDrop(pub tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub trait PeerCount {
    fn peers(&self) -> &AtomicUsize;
}

pub struct PeerGauge<C: PeerCount> {
    count: C,
    counted: usize,
}

impl<C: PeerCount> PeerGauge<C> {
    pub fn new(count: C) -> Self {
        Self { count, counted: 0 }
    }

    pub fn set(&mut self, now: usize) {
        if now > self.counted {
            self.count
                .peers()
                .fetch_add(now - self.counted, Ordering::AcqRel);
        } else {
            self.count
                .peers()
                .fetch_sub(self.counted - now, Ordering::AcqRel);
        }
        self.counted = now;
    }
}

impl<C: PeerCount> Drop for PeerGauge<C> {
    fn drop(&mut self) {
        self.set(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Arc;

    fn at(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn the_peer_cap_is_read_for_each_new_peer_and_never_drops_known_ones() {
        let mut p = PeerTable::default();
        assert!(p.id_for(at(1), || 2).is_some());
        assert!(p.id_for(at(2), || 2).is_some());
        assert!(p.id_for(at(3), || 2).is_none());
        assert!(p.id_for(at(3), || 3).is_some());
        assert!(p.id_for(at(4), || 1).is_none());
        for known in 1..=3 {
            assert!(p.id_for(at(known), || 1).is_some());
        }
        assert_eq!(p.len(), 3);
        p.expire(Duration::ZERO);
        assert_eq!(p.len(), 0);
        assert!(p.id_for(at(5), || 1).is_some());
        assert!(p.id_for(at(6), || 1).is_none());
    }

    #[test]
    fn ids_map_back_to_their_keys_until_the_key_is_dropped() {
        let mut p = PeerTable::<u32>::default();
        let a = p.id_for(70, || 10).unwrap();
        let b = p.id_for(80, || 10).unwrap();
        assert_ne!(a, b);
        assert_eq!(p.id_for(70, || 0), Some(a));
        assert_eq!(p.touch(a), Some(70));
        assert_eq!(p.touch(b), Some(80));
        p.retain(|k| k != 70);
        assert_eq!(p.touch(a), None);
        assert_eq!(p.touch(b), Some(80));
        assert_eq!(p.len(), 1);
        assert!(!p.is_empty());
    }

    #[test]
    fn a_gauge_adds_what_it_counts_and_takes_it_back_when_dropped() {
        struct Count(Arc<AtomicUsize>);
        impl PeerCount for Count {
            fn peers(&self) -> &AtomicUsize {
                &self.0
            }
        }
        let total = Arc::new(AtomicUsize::new(5));
        let mut gauge = PeerGauge::new(Count(total.clone()));
        gauge.set(3);
        assert_eq!(total.load(Ordering::Acquire), 8);
        gauge.set(1);
        assert_eq!(total.load(Ordering::Acquire), 6);
        drop(gauge);
        assert_eq!(total.load(Ordering::Acquire), 5);
    }
}
