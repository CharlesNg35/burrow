use std::time::Duration;

const MIN: u64 = 60;
const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

pub const BACKOFF_SECS: [u64; 4] = [MIN, 10 * MIN, 100 * MIN, DAY];
pub const SHORT_LIVED_FLOOR: u64 = 15 * MIN;
pub const ARI_MIN: Duration = Duration::from_secs(MIN);
pub const ARI_MAX: Duration = Duration::from_secs(12 * HOUR);
pub const ARI_LONG_TERM_ERROR: Duration = Duration::from_secs(6 * HOUR);

pub fn short_lived(not_before: u64, not_after: u64) -> bool {
    not_after.saturating_sub(not_before) < 10 * DAY
}

pub fn renew_at(not_before: u64, not_after: u64) -> u64 {
    let life = not_after.saturating_sub(not_before);
    if short_lived(not_before, not_after) {
        not_before + life / 2
    } else {
        not_before + life / 3 * 2
    }
}

pub fn backoff(failures: u32, short_lived: bool, left: Option<u64>) -> Duration {
    let step = failures.saturating_sub(1).min(3) as usize;
    let mut secs = BACKOFF_SECS[step];
    if short_lived {
        secs = if left.is_none_or(|l| l > 2 * DAY) {
            secs.max(6 * HOUR)
        } else {
            secs.clamp(SHORT_LIVED_FLOOR, 4 * HOUR)
        };
    }
    Duration::from_secs(secs)
}

pub fn jittered(d: Duration, fraction: f64) -> Duration {
    d + d.mul_f64(fraction.clamp(0.0, 1.0) / 10.0)
}

pub fn after_failure(
    failures: u32,
    short_lived: bool,
    left: Option<u64>,
    told_to_wait: Option<Duration>,
) -> Duration {
    let own = backoff(failures, short_lived, left);
    told_to_wait.map_or(own, |w| own.max(w))
}

pub fn ari_next_check(retry_after: Option<Duration>) -> Duration {
    retry_after.map_or(ARI_LONG_TERM_ERROR, |r| r.clamp(ARI_MIN, ARI_MAX))
}

pub fn in_window(start: u64, end: u64, fraction: f64) -> u64 {
    let span = end.saturating_sub(start);
    start + (span as f64 * fraction.clamp(0.0, 1.0)) as u64
}

pub fn urgent(left: u64, short_lived: bool) -> bool {
    left < if short_lived { 2 * DAY } else { 7 * DAY }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NB: u64 = 1_800_000_000;

    #[test]
    fn renewal_comes_at_two_thirds_of_the_life_and_at_half_for_short_lived_certificates() {
        assert_eq!(renew_at(NB, NB + 90 * DAY), NB + 60 * DAY);
        assert_eq!(renew_at(NB, NB + 45 * DAY), NB + 30 * DAY);
        assert_eq!(renew_at(NB, NB + 160 * HOUR), NB + 80 * HOUR);
        assert!(short_lived(NB, NB + 160 * HOUR));
        assert!(!short_lived(NB, NB + 45 * DAY));
    }

    #[test]
    fn failures_back_off_like_lets_encrypt_asks_then_daily() {
        let got: Vec<u64> = (1..=6).map(|n| backoff(n, false, None).as_secs()).collect();
        assert_eq!(got, [60, 600, 6000, DAY, DAY, DAY]);
    }

    #[test]
    fn short_lived_certificates_retry_4_times_a_day_early_then_every_15_minutes_to_4_hours() {
        assert_eq!(backoff(1, true, Some(5 * DAY)).as_secs(), 6 * HOUR);
        assert_eq!(backoff(1, true, Some(DAY)).as_secs(), 15 * MIN);
        assert_eq!(backoff(3, true, Some(DAY)).as_secs(), 6000);
        assert_eq!(backoff(4, true, Some(DAY)).as_secs(), 4 * HOUR);
    }

    #[test]
    fn a_rate_limit_waits_at_least_as_long_as_the_server_says() {
        let told = Some(Duration::from_secs(3 * HOUR));
        assert_eq!(after_failure(1, false, None, told).as_secs(), 3 * HOUR);
        assert_eq!(after_failure(4, false, None, told).as_secs(), DAY);
        assert_eq!(after_failure(1, false, None, None).as_secs(), 60);
    }

    #[test]
    fn the_ari_check_follows_retry_after_within_a_minute_and_twelve_hours() {
        assert_eq!(ari_next_check(Some(Duration::from_secs(5))), ARI_MIN);
        assert_eq!(
            ari_next_check(Some(Duration::from_secs(21_600))).as_secs(),
            21_600
        );
        assert_eq!(ari_next_check(Some(Duration::from_secs(10 * DAY))), ARI_MAX);
        assert_eq!(ari_next_check(None), ARI_LONG_TERM_ERROR);
    }

    #[test]
    fn jitter_adds_at_most_a_tenth_and_window_picks_stay_inside() {
        let d = Duration::from_secs(600);
        assert_eq!(jittered(d, 0.0), d);
        assert_eq!(jittered(d, 1.0).as_secs(), 660);
        assert_eq!(in_window(100, 200, 0.5), 150);
        assert_eq!(in_window(100, 200, 7.0), 200);
        assert_eq!(in_window(200, 100, 0.5), 200);
    }

    #[test]
    fn little_time_left_is_an_error() {
        assert!(urgent(6 * DAY, false));
        assert!(!urgent(8 * DAY, false));
        assert!(urgent(DAY, true));
        assert!(!urgent(3 * DAY, true));
    }
}
