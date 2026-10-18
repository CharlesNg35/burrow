use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct Throttle {
    state: Mutex<Option<(Instant, Duration)>>,
    skipped: AtomicUsize,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new()
    }
}

impl Throttle {
    const EVERY: Duration = Duration::from_secs(10);
    const LONGEST: Duration = Duration::from_secs(600);

    pub const fn new() -> Self {
        Throttle {
            state: Mutex::new(None),
            skipped: AtomicUsize::new(0),
        }
    }

    pub fn allow(&self) -> Option<String> {
        let now = Instant::now();
        let mut state = self.state.lock().expect("throttle lock");
        let (last, every) = match *state {
            Some((last, every)) if now < last + every => {
                self.skipped.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            Some((last, every)) if now < last + every * 2 => {
                (Some(every), (every * 6).min(Self::LONGEST))
            }
            Some((_, every)) => (Some(every), Self::EVERY),
            None => (None, Self::EVERY),
        };
        *state = Some((now, every));
        Some(match (self.skipped.swap(0, Ordering::Relaxed), last) {
            (0, _) | (_, None) => String::new(),
            (n, Some(window)) => format!(" ({n} more like this in the last {})", shown(window)),
        })
    }
}

fn shown(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => format!("{s}s"),
        s => format!("{} min", s / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewind(t: &Throttle, by: Duration) {
        let mut state = t.state.lock().unwrap();
        let (last, every) = state.unwrap();
        *state = Some((last - by, every));
    }

    #[test]
    fn throttled_warnings_count_what_they_skip() {
        let t = Throttle::new();
        assert_eq!(t.allow().as_deref(), Some(""));
        for _ in 0..5 {
            assert_eq!(t.allow(), None);
        }
        rewind(&t, Duration::from_secs(10));
        assert_eq!(
            t.allow().as_deref(),
            Some(" (5 more like this in the last 10s)")
        );
        assert_eq!(t.allow(), None);
    }

    #[test]
    fn a_warning_that_keeps_coming_back_is_logged_less_and_less_often() {
        let t = Throttle::new();
        let every = |t: &Throttle| t.state.lock().unwrap().unwrap().1;
        t.allow();
        assert_eq!(every(&t), Duration::from_secs(10));
        rewind(&t, Duration::from_secs(10));
        t.allow();
        assert_eq!(every(&t), Duration::from_secs(60));
        rewind(&t, Duration::from_secs(60));
        t.allow();
        assert_eq!(every(&t), Duration::from_secs(360));
        rewind(&t, Duration::from_secs(360));
        t.allow();
        assert_eq!(every(&t), Duration::from_secs(600));
        rewind(&t, Duration::from_secs(1300));
        t.allow();
        assert_eq!(every(&t), Duration::from_secs(10));
    }
}
