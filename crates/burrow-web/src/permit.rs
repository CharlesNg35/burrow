use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

type Sources = Arc<Mutex<HashMap<IpAddr, usize>>>;

pub struct Permits {
    slots: RwLock<(Arc<Semaphore>, usize)>,
    sources: Sources,
    refused: AtomicU64,
}

pub struct Permit {
    _slot: OwnedSemaphorePermit,
    _source: Option<SourceSlot>,
}

struct SourceSlot {
    sources: Sources,
    key: IpAddr,
}

impl Drop for SourceSlot {
    fn drop(&mut self) {
        let mut sources = self.sources.lock().expect("sources lock");
        if let Some(n) = sources.get_mut(&self.key) {
            *n -= 1;
            if *n == 0 {
                sources.remove(&self.key);
            }
        }
    }
}

pub fn source_key(ip: IpAddr) -> Option<IpAddr> {
    let ip = ip.to_canonical();
    if ip.is_loopback() {
        return None;
    }
    Some(match ip {
        IpAddr::V6(v6) => IpAddr::V6((v6.to_bits() >> 64 << 64).into()),
        v4 => v4,
    })
}

impl Permits {
    pub fn new(total: usize) -> Permits {
        Permits {
            slots: RwLock::new((Arc::new(Semaphore::new(total)), total)),
            sources: Sources::default(),
            refused: AtomicU64::new(0),
        }
    }

    pub fn resize(&self, total: usize) {
        let mut slots = self.slots.write().expect("slots lock");
        if slots.1 != total {
            *slots = (Arc::new(Semaphore::new(total)), total);
        }
    }

    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    pub fn try_take(&self, ip: IpAddr) -> Option<Permit> {
        self.take(source_key(ip))
    }

    pub fn try_take_shared(&self) -> Option<Permit> {
        self.take(None)
    }

    fn take(&self, key: Option<IpAddr>) -> Option<Permit> {
        let (slots, total) = self.slots.read().expect("slots lock").clone();
        let pressed = slots.available_permits() < total / 2;
        let source = match self.source_slot(key, total, pressed) {
            Ok(s) => s,
            Err(()) => {
                self.refused.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        match slots.try_acquire_owned() {
            Ok(slot) => Some(Permit {
                _slot: slot,
                _source: source,
            }),
            Err(_) => {
                self.refused.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    pub fn claim_source(&self, permit: &mut Permit, ip: IpAddr) -> bool {
        let (slots, total) = self.slots.read().expect("slots lock").clone();
        let pressed = slots.available_permits() < total / 2;
        match self.source_slot(source_key(ip), total, pressed) {
            Ok(source) => {
                permit._source = source;
                true
            }
            Err(()) => {
                self.refused.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    fn source_slot(
        &self,
        key: Option<IpAddr>,
        total: usize,
        pressed: bool,
    ) -> Result<Option<SourceSlot>, ()> {
        let Some(key) = key else {
            return Ok(None);
        };
        let most = (total / 8).max(8);
        let mut held = self.sources.lock().expect("sources lock");
        let n = held.entry(key).or_insert(0);
        if pressed && *n >= most {
            return Err(());
        }
        *n += 1;
        Ok(Some(SourceSlot {
            sources: self.sources.clone(),
            key,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn one_address_cannot_take_every_slot_once_the_port_is_busy() {
        let p = Permits::new(256);
        let calm: Vec<_> = (0..129)
            .map(|_| p.try_take(ip("203.0.113.7")).unwrap())
            .collect();
        assert!(p.try_take(ip("203.0.113.7")).is_none());
        assert!(p.try_take(ip("203.0.113.8")).is_some());
        let net: Vec<_> = (0..32)
            .map(|i| p.try_take(ip(&format!("2001:db8::{i:x}"))).unwrap())
            .collect();
        assert!(p.try_take(ip("2001:db8::ffff:1")).is_none());
        assert!(p.try_take(ip("::1")).is_some());
        assert!(p.try_take_shared().is_some());
        assert_eq!(p.refused(), 2);
        drop(calm);
        drop(net);
        assert!(p.sources.lock().unwrap().is_empty());
    }

    #[test]
    fn a_visitor_behind_a_balancer_gets_the_same_share_as_a_direct_one() {
        let p = Permits::new(16);
        let mut held = Vec::new();
        let mut granted = 0;
        for _ in 0..12 {
            let mut permit = p.try_take_shared().unwrap();
            if p.claim_source(&mut permit, ip("203.0.113.7")) {
                granted += 1;
                held.push(permit);
            }
        }
        assert_eq!(granted, 8);
        assert_eq!(p.refused(), 4);
        let mut other = p.try_take_shared().unwrap();
        assert!(p.claim_source(&mut other, ip("203.0.113.8")));
        drop(held);
        drop(other);
        assert!(p.sources.lock().unwrap().is_empty());
    }

    #[test]
    fn the_cap_holds_and_a_resize_takes_effect_for_new_connections() {
        let p = Permits::new(2);
        let a = p.try_take(ip("127.0.0.1")).unwrap();
        let _b = p.try_take(ip("127.0.0.1")).unwrap();
        assert!(p.try_take(ip("127.0.0.1")).is_none());
        p.resize(3);
        let _c = p.try_take(ip("127.0.0.1")).unwrap();
        drop(a);
        p.resize(3);
        assert!(p.try_take(ip("127.0.0.1")).is_some());
    }
}
