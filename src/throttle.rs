use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct Throttle {
    next: Mutex<Option<Instant>>,
    skipped: AtomicUsize,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new()
    }
}

impl Throttle {
    const EVERY: Duration = Duration::from_secs(10);

    pub const fn new() -> Self {
        Throttle {
            next: Mutex::new(None),
            skipped: AtomicUsize::new(0),
        }
    }

    pub fn allow(&self) -> Option<String> {
        let now = Instant::now();
        let mut next = self.next.lock().expect("throttle lock");
        if next.is_some_and(|t| now < t) {
            self.skipped.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        *next = Some(now + Self::EVERY);
        Some(match self.skipped.swap(0, Ordering::Relaxed) {
            0 => String::new(),
            n => format!(
                " ({n} more like this in the last {}s)",
                Self::EVERY.as_secs()
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttled_warnings_count_what_they_skip() {
        let t = Throttle::new();
        assert_eq!(t.allow().as_deref(), Some(""));
        for _ in 0..5 {
            assert_eq!(t.allow(), None);
        }
        *t.next.lock().unwrap() = Some(Instant::now());
        assert_eq!(
            t.allow().as_deref(),
            Some(" (5 more like this in the last 10s)")
        );
        assert_eq!(t.allow(), None);
    }
}
