pub const DAY_SECS: u64 = 86_400;

pub fn days_left(at: u64, now: u64) -> String {
    match at.saturating_sub(now) / DAY_SECS {
        1 => "1 day".to_string(),
        n => format!("{n} days"),
    }
}

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
    let (d, h, m, s) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, _) => format!("{m}m"),
        (0, _, 0) => format!("{h}h"),
        (0, _, _) => format!("{h}h {m}m"),
        (_, 0, _) => format!("{d}d"),
        _ => format!("{d}d {h}h"),
    }
}

pub fn ago(secs: u64) -> String {
    if secs < 5 {
        "just now".into()
    } else {
        format!("{} ago", span(secs))
    }
}

pub fn human(value: u64, stored: TimeUnit) -> String {
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
        (Some(a), Some(b)) => format!("between {} and {}", human(a, stored), human(b, stored)),
        (Some(a), None) => format!("at least {}", human(a, stored)),
        (None, Some(b)) => format!("at most {}", human(b, stored)),
        (None, None) => String::new(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn days_left_says_one_day_and_counts_the_rest() {
        use super::{DAY_SECS, days_left};
        assert_eq!(days_left(DAY_SECS + 5, 0), "1 day");
        assert_eq!(days_left(2 * DAY_SECS + 5, 0), "2 days");
        assert_eq!(days_left(100, 0), "0 days");
    }

    use super::*;

    #[test]
    fn a_span_reads_like_2h_5m_with_its_two_largest_units() {
        assert_eq!(span(0), "0s");
        assert_eq!(span(59), "59s");
        assert_eq!(span(125), "2m");
        assert_eq!(span(7500), "2h 5m");
        assert_eq!(span(3 * 86_400 + 4 * 3600 + 59), "3d 4h");
        assert_eq!(span(7200 + 59), "2h");
        assert_eq!(span(60 * 86_400 + 1800), "60d");
        assert_eq!(ago(4), "just now");
        assert_eq!(ago(7500), "2h 5m ago");
    }

    #[test]
    fn conversions() {
        assert_eq!(human(5400, TimeUnit::Secs), "1h 30m");
        assert_eq!(human(0, TimeUnit::Secs), "0s");
        assert_eq!(human(1500, TimeUnit::Ms), "1s 500ms");
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
