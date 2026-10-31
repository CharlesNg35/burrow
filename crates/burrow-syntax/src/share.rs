use std::collections::HashMap;
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub fn subnet_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6((v6.to_bits() >> 64 << 64).into()),
        v4 => v4,
    }
}

pub fn source_key(ip: IpAddr) -> Option<IpAddr> {
    (!ip.to_canonical().is_loopback()).then(|| subnet_key(ip))
}

pub fn fair_part(total: usize) -> usize {
    (total / 8).max(8)
}

pub fn pressed(total: usize, used: usize) -> bool {
    total.saturating_sub(used) < total / 2
}

type Held<K> = Arc<Mutex<HashMap<K, usize>>>;

fn locked<K>(held: &Held<K>) -> MutexGuard<'_, HashMap<K, usize>> {
    held.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Shares<K> {
    held: Held<K>,
}

impl<K> Default for Shares<K> {
    fn default() -> Self {
        Shares {
            held: Arc::default(),
        }
    }
}

pub struct Share<K: Eq + Hash> {
    held: Held<K>,
    key: K,
}

impl<K: Eq + Hash> Drop for Share<K> {
    fn drop(&mut self) {
        let mut held = locked(&self.held);
        if let Some(n) = held.get_mut(&self.key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                held.remove(&self.key);
            }
        }
    }
}

impl<K: Eq + Hash + Clone> Shares<K> {
    pub fn take(&self, key: K, total: usize, used: usize) -> Option<Share<K>> {
        let mut held = locked(&self.held);
        let n = held.entry(key.clone()).or_insert(0);
        if pressed(total, used) && *n >= fair_part(total) {
            return None;
        }
        *n += 1;
        Some(Share {
            held: self.held.clone(),
            key,
        })
    }

    pub fn is_empty(&self) -> bool {
        locked(&self.held).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn an_address_is_held_to_its_part_only_once_the_pool_is_half_used() {
        let shares = Shares::default();
        let a = ip("203.0.113.7");
        let calm: Vec<_> = (0..50).map(|_| shares.take(a, 100, 49).unwrap()).collect();
        assert!(
            shares.take(a, 100, 50).is_some(),
            "half used is not pressed"
        );
        assert!(
            shares.take(a, 100, 51).is_none(),
            "past half: over its part"
        );
        assert!(shares.take(ip("203.0.113.8"), 100, 99).is_some());
        drop(calm);
        assert!(shares.is_empty());
    }

    #[test]
    fn the_part_is_an_eighth_and_never_below_eight() {
        assert_eq!(fair_part(8192), 1024);
        assert_eq!(fair_part(256), 32);
        assert_eq!(fair_part(16), 8);
        assert_eq!(fair_part(1), 8);
    }

    #[test]
    fn an_ipv6_slash_64_is_one_address_and_loopback_has_no_key() {
        assert_eq!(
            source_key(ip("2001:db8::1")),
            source_key(ip("2001:db8::ffff:2"))
        );
        assert_ne!(
            source_key(ip("2001:db8::1")),
            source_key(ip("2001:db8:0:1::1"))
        );
        assert_eq!(source_key(ip("::1")), None);
        assert_eq!(source_key(ip("127.0.0.9")), None);
        assert_eq!(source_key(ip("::ffff:127.0.0.1")), None);
        assert_eq!(
            source_key(ip("::ffff:203.0.113.7")),
            Some(ip("203.0.113.7"))
        );
    }

    #[test]
    fn a_slash_64_shares_one_part_and_every_share_is_given_back() {
        let shares = Shares::default();
        let key = |s: &str| source_key(ip(s)).unwrap();
        let same_net: Vec<_> = (0..32)
            .map(|i| {
                shares
                    .take(key(&format!("2001:db8::{i:x}")), 256, 200)
                    .unwrap()
            })
            .collect();
        assert!(shares.take(key("2001:db8::ffff:1"), 256, 200).is_none());
        assert!(shares.take(key("2001:db8:0:1::1"), 256, 200).is_some());
        drop(same_net);
        assert!(shares.is_empty(), "every share is given back");
        assert!(shares.take(key("2001:db8::1"), 256, 200).is_some());
    }
}
