use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ring::hmac;

pub const KEEP: Duration = Duration::from_secs(600);
pub const MOST: usize = 1024;

type Tag = [u8; 32];

pub(crate) struct Cache {
    key: hmac::Key,
    held: Mutex<HashMap<Tag, Instant>>,
    keep: Duration,
    most: usize,
}

impl Cache {
    pub(crate) fn new() -> Cache {
        Cache::sized(KEEP, MOST)
    }

    pub(crate) fn sized(keep: Duration, most: usize) -> Cache {
        let rng = ring::rand::SystemRandom::new();
        Cache {
            key: hmac::Key::generate(hmac::HMAC_SHA256, &rng).expect("the system has randomness"),
            held: Mutex::new(HashMap::new()),
            keep,
            most,
        }
    }

    pub(crate) fn tag(&self, user: &str, password: &[u8], hash: &str) -> Tag {
        let mut ctx = hmac::Context::with_key(&self.key);
        for field in [user.as_bytes(), password, hash.as_bytes()] {
            ctx.update(&(field.len() as u64).to_be_bytes());
            ctx.update(field);
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(ctx.sign().as_ref());
        out
    }

    pub(crate) fn hit(&self, tag: &Tag, now: Instant) -> bool {
        let mut held = self.held.lock().expect("cache lock");
        match held.get(tag) {
            Some(at) if now.saturating_duration_since(*at) < self.keep => true,
            Some(_) => {
                held.remove(tag);
                false
            }
            None => false,
        }
    }

    pub(crate) fn keep(&self, tag: Tag, now: Instant) {
        let mut held = self.held.lock().expect("cache lock");
        if held.len() >= self.most && !held.contains_key(&tag) {
            held.retain(|_, at| now.saturating_duration_since(*at) < self.keep);
            if held.len() >= self.most
                && let Some(oldest) = held.iter().min_by_key(|(_, at)| **at).map(|(t, _)| *t)
            {
                held.remove(&oldest);
            }
        }
        held.insert(tag, now);
    }

    pub(crate) fn sweep(&self, now: Instant) {
        let mut held = self.held.lock().expect("cache lock");
        held.retain(|_, at| now.saturating_duration_since(*at) < self.keep);
        held.shrink_to_fit();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.held.lock().expect("cache lock").len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_length_prefixed_so_shifted_splits_never_meet() {
        let c = Cache::new();
        assert_ne!(c.tag("ab", b"c", "h"), c.tag("a", b"bc", "h"));
        assert_ne!(c.tag("a", b"b", "ch"), c.tag("a", b"bc", "h"));
        assert_eq!(c.tag("a", b"b", "h"), c.tag("a", b"b", "h"));
        assert_ne!(c.tag("a", b"b", "h"), Cache::new().tag("a", b"b", "h"));
    }

    #[test]
    fn a_success_is_kept_ten_minutes_and_a_changed_hash_misses() {
        let c = Cache::new();
        let t0 = Instant::now();
        let tag = c.tag("bob", b"pw", "hash1");
        assert!(!c.hit(&tag, t0));
        c.keep(tag, t0);
        assert!(c.hit(&tag, t0 + Duration::from_secs(599)));
        assert!(!c.hit(&c.tag("bob", b"pw", "hash2"), t0));
        assert!(!c.hit(&tag, t0 + KEEP));
        assert_eq!(c.len(), 0);
    }

    #[test]
    fn the_cache_never_grows_past_its_cap_and_the_sweep_empties_it() {
        let c = Cache::sized(KEEP, 8);
        let t0 = Instant::now();
        for i in 0..100u32 {
            c.keep(
                c.tag("u", &i.to_be_bytes(), "h"),
                t0 + Duration::from_millis(i.into()),
            );
            assert!(c.len() <= 8);
        }
        assert!(c.hit(&c.tag("u", &99u32.to_be_bytes(), "h"), t0));
        c.sweep(t0 + KEEP + Duration::from_secs(1));
        assert_eq!(c.len(), 0);
    }
}
