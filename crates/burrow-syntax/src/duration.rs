pub const DAY_SECS: u64 = 86_400;

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    Ms,
    Secs,
    Mins,
    Hours,
}

impl TimeUnit {
    pub const ALL: [TimeUnit; 4] = [
        TimeUnit::Ms,
        TimeUnit::Secs,
        TimeUnit::Mins,
        TimeUnit::Hours,
    ];

    pub fn ms(self) -> u64 {
        match self {
            TimeUnit::Ms => 1,
            TimeUnit::Secs => 1000,
            TimeUnit::Mins => 60_000,
            TimeUnit::Hours => 3_600_000,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimeUnit::Ms => "milliseconds",
            TimeUnit::Secs => "seconds",
            TimeUnit::Mins => "minutes",
            TimeUnit::Hours => "hours",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            TimeUnit::Ms => "ms",
            TimeUnit::Secs => "s",
            TimeUnit::Mins => "min",
            TimeUnit::Hours => "h",
        }
    }
}

pub fn units_for(stored: TimeUnit, max_stored: Option<u64>) -> Vec<TimeUnit> {
    let max_ms = max_stored.map(|m| m.saturating_mul(stored.ms()));
    TimeUnit::ALL
        .into_iter()
        .filter(|u| u.ms() >= stored.ms())
        .filter(|u| max_ms.is_none_or(|m| u.ms() <= m.max(stored.ms())))
        .collect()
}

pub fn best_unit(value: u64, stored: TimeUnit, units: &[TimeUnit]) -> TimeUnit {
    let ms = value.saturating_mul(stored.ms());
    units
        .iter()
        .rev()
        .copied()
        .find(|u| ms > 0 && ms.is_multiple_of(u.ms()))
        .unwrap_or(stored)
}

pub fn to_stored(number: &str, unit: TimeUnit, stored: TimeUnit) -> Result<u64, String> {
    let n: u64 = number
        .trim()
        .parse()
        .map_err(|_| format!("\"{}\" is not a whole number", number.trim()))?;
    let ms = n
        .checked_mul(unit.ms())
        .ok_or_else(|| "the duration is too long".to_string())?;
    if !ms.is_multiple_of(stored.ms()) {
        return Err(format!("use whole {}", stored.label()));
    }
    Ok(ms / stored.ms())
}

fn suffix(u: TimeUnit) -> &'static str {
    match u {
        TimeUnit::Ms => "ms",
        TimeUnit::Secs => "s",
        TimeUnit::Mins => "m",
        TimeUnit::Hours => "h",
    }
}

pub fn span(secs: u64) -> String {
    let units = [
        (secs / 86_400, "d"),
        (secs / 3600 % 24, "h"),
        (secs / 60 % 60, "m"),
        (secs % 60, "s"),
    ];
    let Some(top) = units.iter().position(|(n, _)| *n > 0) else {
        return "0s".into();
    };
    let (n, u) = units[top];
    match units.get(top + 1) {
        Some((m, v)) if *m > 0 => format!("{n}{u} {m}{v}"),
        _ => format!("{n}{u}"),
    }
}

pub fn ago(secs: u64) -> String {
    if secs < 5 {
        JUST_NOW.into()
    } else {
        format!("{} ago", span(secs))
    }
}

pub const JUST_NOW: &str = "just now";
pub const UNDER_A_MINUTE: &str = "less than a minute";

pub fn coarse(secs: u64) -> String {
    if secs < 60 {
        JUST_NOW.into()
    } else {
        span(secs - secs % 60)
    }
}

pub fn coarse_length(secs: u64) -> String {
    if secs < 60 {
        UNDER_A_MINUTE.into()
    } else {
        coarse(secs)
    }
}

pub fn coarse_ago(secs: u64) -> String {
    if secs < 60 {
        JUST_NOW.into()
    } else {
        format!("{} ago", coarse(secs))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Elapsed {
    #[default]
    Exact,
    Coarse,
}

impl Elapsed {
    pub fn length(self, secs: u64) -> String {
        match self {
            Elapsed::Exact => span(secs),
            Elapsed::Coarse => coarse_length(secs),
        }
    }

    pub fn ago(self, secs: u64) -> String {
        match self {
            Elapsed::Exact => ago(secs),
            Elapsed::Coarse => coarse_ago(secs),
        }
    }
}

pub fn exact(value: u64, stored: TimeUnit) -> String {
    let mut ms = value.saturating_mul(stored.ms());
    if ms == 0 {
        return format!("0{}", suffix(stored));
    }
    let mut parts = Vec::new();
    for u in [
        TimeUnit::Hours,
        TimeUnit::Mins,
        TimeUnit::Secs,
        TimeUnit::Ms,
    ] {
        let n = ms / u.ms();
        if n > 0 {
            parts.push(format!("{n}{}", suffix(u)));
            ms -= n * u.ms();
        }
    }
    parts.join(" ")
}

pub fn check_range(
    value: u64,
    min: Option<u64>,
    max: Option<u64>,
    stored: TimeUnit,
) -> Result<(), String> {
    let out = min.is_some_and(|m| value < m) || max.is_some_and(|m| value > m);
    if !out {
        return Ok(());
    }
    Err(match (min, max) {
        (Some(a), Some(b)) => format!("between {} and {}", exact(a, stored), exact(b, stored)),
        (Some(a), None) => format!("at least {}", exact(a, stored)),
        (None, Some(b)) => format!("at most {}", exact(b, stored)),
        (None, None) => String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_reads_like_2h_5m_with_its_two_largest_units() {
        assert_eq!(span(0), "0s");
        assert_eq!(span(45), "45s");
        assert_eq!(span(200), "3m 20s");
        assert_eq!(span(180), "3m");
        assert_eq!(span(8040), "2h 14m");
        assert_eq!(span(7200 + 59), "2h");
        assert_eq!(span(90_000), "1d 1h");
        assert_eq!(span(60 * 86_400 + 1800), "60d");
        assert_eq!(ago(4), "just now");
        assert_eq!(ago(7500), "2h 5m ago");
    }

    #[test]
    fn coarse_reads_like_5m_and_never_shows_seconds() {
        assert_eq!(coarse(0), "just now");
        assert_eq!(coarse(59), "just now");
        assert_eq!(coarse(60), "1m");
        assert_eq!(coarse(3599), "59m");
        assert_eq!(coarse(3600), "1h");
        assert_eq!(coarse(3600 + 5 * 60 + 59), "1h 5m");
        assert_eq!(coarse(86_399), "23h 59m");
        assert_eq!(coarse(86_400), "1d");
        assert_eq!(coarse(90_000), "1d 1h");
        assert_eq!(coarse(3 * 86_400 + 4 * 3600 + 59 * 60), "3d 4h");
        assert_eq!(coarse_length(59), "less than a minute");
        assert_eq!(coarse_length(7500), "2h 5m");
    }

    #[test]
    fn coarse_ago_says_just_now_under_a_minute() {
        assert_eq!(coarse_ago(0), "just now");
        assert_eq!(coarse_ago(59), "just now");
        assert_eq!(coarse_ago(60), "1m ago");
        assert_eq!(coarse_ago(3599), "59m ago");
        assert_eq!(coarse_ago(7500), "2h 5m ago");
        assert_eq!(coarse_ago(90_000), "1d 1h ago");
    }

    #[test]
    fn elapsed_picks_the_precise_or_the_coarse_form() {
        assert_eq!(Elapsed::Exact.length(150), "2m 30s");
        assert_eq!(Elapsed::Coarse.length(150), "2m");
        assert_eq!(Elapsed::Coarse.length(30), "less than a minute");
        assert_eq!(Elapsed::Exact.ago(150), "2m 30s ago");
        assert_eq!(Elapsed::Coarse.ago(30), "just now");
    }

    #[test]
    fn exact_writes_every_unit_while_span_keeps_the_two_largest() {
        assert_eq!(exact(3 * 3600 + 25 * 60 + 7, TimeUnit::Secs), "3h 25m 7s");
        assert_eq!(span(3 * 3600 + 25 * 60 + 7), "3h 25m");
    }

    #[test]
    fn conversions() {
        assert_eq!(exact(5400, TimeUnit::Secs), "1h 30m");
        assert_eq!(exact(0, TimeUnit::Secs), "0s");
        assert_eq!(exact(1500, TimeUnit::Ms), "1s 500ms");
        assert_eq!(to_stored("2", TimeUnit::Mins, TimeUnit::Secs).unwrap(), 120);
        assert!(to_stored("x", TimeUnit::Secs, TimeUnit::Secs).is_err());
        assert!(to_stored("1500", TimeUnit::Ms, TimeUnit::Secs).is_err());
        assert_eq!(
            best_unit(120, TimeUnit::Secs, &units_for(TimeUnit::Secs, Some(3600))),
            TimeUnit::Mins
        );
        assert_eq!(
            units_for(TimeUnit::Secs, Some(60)),
            vec![TimeUnit::Secs, TimeUnit::Mins]
        );
        assert_eq!(
            check_range(0, Some(1), Some(3600), TimeUnit::Secs).unwrap_err(),
            "between 1s and 1h"
        );
        assert!(check_range(10, Some(1), Some(3600), TimeUnit::Secs).is_ok());
    }
}
