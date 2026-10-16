use std::time::{Duration, Instant};

pub const SUMMARY_EVERY: Duration = Duration::from_secs(60);

pub struct Backoff {
    initial: Duration,
    max: Duration,
    current: Duration,
}

impl Backoff {
    pub fn new(initial: Duration, max: Duration) -> Self {
        Backoff {
            initial,
            max,
            current: initial,
        }
    }

    pub fn reset(&mut self) {
        self.current = self.initial;
    }

    pub fn next_delay(&mut self) -> Duration {
        let base = self.current;
        self.current = (self.current * 2).min(self.max);
        let mut b = [0u8; 4];
        getrandom::fill(&mut b).expect("operating system random source is available");
        let frac = f64::from(u32::from_le_bytes(b)) / f64::from(u32::MAX);
        base.mul_f64(0.5 + frac * 0.5)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    First,
    Summary { down_for: Duration, attempts: u64 },
    Quiet,
}

#[derive(Debug, Default)]
pub struct Outage {
    since: Option<Instant>,
    attempts: u64,
    last_report: Option<Instant>,
}

impl Outage {
    pub fn failed(&mut self, now: Instant) -> Report {
        self.attempts += 1;
        let since = *self.since.get_or_insert(now);
        match self.last_report {
            None => {
                self.last_report = Some(now);
                Report::First
            }
            Some(last) if now.duration_since(last) >= SUMMARY_EVERY => {
                self.last_report = Some(now);
                Report::Summary {
                    down_for: now.duration_since(since),
                    attempts: self.attempts,
                }
            }
            Some(_) => Report::Quiet,
        }
    }

    pub fn down_for(&self, now: Instant) -> Duration {
        self.since.map_or(Duration::ZERO, |s| now.duration_since(s))
    }

    pub fn recovered(&mut self, now: Instant) -> Option<(Duration, u64)> {
        let since = self.since.take()?;
        let attempts = std::mem::take(&mut self.attempts);
        self.last_report = None;
        Some((now.duration_since(since), attempts))
    }
}

pub fn human(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        3600..86400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86400, s % 86400 / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_with_jitter_and_caps() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(1));
        let d: Vec<Duration> = (0..8).map(|_| b.next_delay()).collect();
        assert!(d[0] >= Duration::from_millis(50) && d[0] <= Duration::from_millis(100));
        assert!(d[3] >= Duration::from_millis(400) && d[3] <= Duration::from_millis(800));
        assert!(d.iter().all(|x| *x <= Duration::from_secs(1)));
        b.reset();
        assert!(b.next_delay() <= Duration::from_millis(100));
    }

    #[test]
    fn outage_reports_first_then_once_a_minute() {
        let t0 = Instant::now();
        let mut o = Outage::default();
        assert_eq!(o.failed(t0), Report::First);
        assert_eq!(o.failed(t0 + Duration::from_secs(5)), Report::Quiet);
        assert_eq!(o.failed(t0 + Duration::from_secs(59)), Report::Quiet);
        assert_eq!(
            o.failed(t0 + Duration::from_secs(61)),
            Report::Summary {
                down_for: Duration::from_secs(61),
                attempts: 4
            }
        );
        assert_eq!(o.failed(t0 + Duration::from_secs(90)), Report::Quiet);
        assert_eq!(
            o.down_for(t0 + Duration::from_secs(100)),
            Duration::from_secs(100)
        );
        assert_eq!(
            o.recovered(t0 + Duration::from_secs(120)),
            Some((Duration::from_secs(120), 5))
        );
        assert_eq!(o.recovered(t0 + Duration::from_secs(121)), None);
        assert_eq!(o.failed(t0 + Duration::from_secs(200)), Report::First);
    }

    #[test]
    fn human_durations() {
        assert_eq!(human(Duration::from_secs(45)), "45s");
        assert_eq!(human(Duration::from_secs(200)), "3m 20s");
        assert_eq!(human(Duration::from_secs(8040)), "2h 14m");
        assert_eq!(human(Duration::from_secs(90000)), "1d 1h");
    }
}
