pub fn parse_rate(text: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    let trimmed = trimmed.strip_suffix("/s").unwrap_or(trimmed);
    let t = match trimmed.strip_suffix("Bps") {
        Some(prefix) => format!("{}b", prefix.to_ascii_lowercase()),
        None => {
            let lower = trimmed.to_ascii_lowercase();
            match lower.strip_suffix("bps") {
                Some(prefix) => format!("{prefix}bit"),
                None => lower,
            }
        }
    };
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (number, unit) = t.split_at(split);
    let value: f64 = number
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite() && *v > 0.0)
        .ok_or_else(|| format!("\"{text}\" does not start with a positive number"))?;
    let bytes = match unit.trim() {
        "bit" => value / 8.0,
        "kbit" => value * 1e3 / 8.0,
        "mbit" => value * 1e6 / 8.0,
        "gbit" => value * 1e9 / 8.0,
        "b" => value,
        "kb" => value * 1e3,
        "mb" => value * 1e6,
        "gb" => value * 1e9,
        other => {
            return Err(format!("\"{other}\" is not a rate unit"));
        }
    };
    if bytes < 1000.0 {
        return Err(format!("{text} is below the minimum of 1kb (8kbit)"));
    }
    Ok(bytes as u64)
}

pub fn describe(bytes_per_sec: u64) -> String {
    let bits = bytes_per_sec as f64 * 8.0;
    for (unit, size) in [("gbit", 1e9), ("mbit", 1e6), ("kbit", 1e3)] {
        if bits >= size {
            let v = bits / size;
            return if (v - v.round()).abs() < 0.05 {
                format!("{}{unit}", v.round())
            } else {
                format!("{v:.1}{unit}")
            };
        }
    }
    format!("{bits}bit")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateUnit {
    Kbit,
    Mbit,
    Gbit,
    KB,
    MB,
    GB,
}

impl RateUnit {
    pub const ALL: [RateUnit; 6] = [
        RateUnit::Kbit,
        RateUnit::Mbit,
        RateUnit::Gbit,
        RateUnit::KB,
        RateUnit::MB,
        RateUnit::GB,
    ];

    pub fn suffix(self) -> &'static str {
        match self {
            RateUnit::Kbit => "kbit",
            RateUnit::Mbit => "mbit",
            RateUnit::Gbit => "gbit",
            RateUnit::KB => "KB",
            RateUnit::MB => "MB",
            RateUnit::GB => "GB",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RateUnit::Kbit => "kbit/s",
            RateUnit::Mbit => "Mbit/s",
            RateUnit::Gbit => "Gbit/s",
            RateUnit::KB => "KB/s",
            RateUnit::MB => "MB/s",
            RateUnit::GB => "GB/s",
        }
    }

    pub fn from_suffix(s: &str) -> Option<RateUnit> {
        RateUnit::ALL.into_iter().find(|u| u.suffix() == s)
    }
}

pub fn split(text: &str) -> Option<(String, RateUnit)> {
    let t = text.trim();
    let at = t.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (n, u) = t.split_at(at);
    let u = u.trim().trim_end_matches("/s");
    let unit = match u.to_ascii_lowercase().as_str() {
        "kbit" | "kbps" => RateUnit::Kbit,
        "mbit" | "mbps" => RateUnit::Mbit,
        "gbit" | "gbps" => RateUnit::Gbit,
        "kb" => RateUnit::KB,
        "mb" => RateUnit::MB,
        "gb" => RateUnit::GB,
        _ => return None,
    };
    let unit = match (u, unit) {
        ("kbps" | "Kbps", _) => RateUnit::Kbit,
        _ => unit,
    };
    Some((n.to_string(), unit))
}

pub fn compose(number: &str, unit: RateUnit) -> String {
    format!("{}{}", number.trim(), unit.suffix())
}

pub fn equivalent(bytes_per_sec: u64) -> String {
    let bits = bytes_per_sec as f64 * 8.0;
    let bytes = bytes_per_sec as f64;
    let pick = |v: f64, units: [(&'static str, f64); 3]| {
        for (u, s) in units {
            if v >= s {
                let x = v / s;
                return if (x - x.round()).abs() < 0.05 {
                    format!("{} {u}", x.round())
                } else {
                    format!("{x:.1} {u}")
                };
            }
        }
        format!("{v} {}", units[2].0)
    };
    format!(
        "{} ≈ {}",
        pick(bits, [("Gbit/s", 1e9), ("Mbit/s", 1e6), ("kbit/s", 1e3)]),
        pick(bytes, [("GB/s", 1e9), ("MB/s", 1e6), ("KB/s", 1e3)])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_parse() {
        assert_eq!(parse_rate("40mbit").unwrap(), 5_000_000);
        assert_eq!(parse_rate("40Mbit/s").unwrap(), 5_000_000);
        assert_eq!(parse_rate("1gbit").unwrap(), 125_000_000);
        assert_eq!(parse_rate("500kbit").unwrap(), 62_500);
        assert_eq!(parse_rate("5MB").unwrap(), 5_000_000);
        assert_eq!(parse_rate("2.5mbps").unwrap(), 312_500);
        assert_eq!(parse_rate("100kb").unwrap(), 100_000);
        assert!(parse_rate("fast").is_err());
        assert!(parse_rate("0mbit").is_err());
        assert!(parse_rate("1kbit").unwrap_err().contains("minimum"));
        assert_eq!(describe(5_000_000), "40mbit");
        assert_eq!(describe(62_500), "500kbit");
        assert_eq!(describe(125_000_000), "1gbit");
        assert_eq!(describe(312_500), "2.5mbit");
    }

    #[test]
    fn bytes_and_bits_per_second_are_told_apart() {
        assert_eq!(parse_rate("5MBps").unwrap(), 5_000_000);
        assert_eq!(parse_rate("5Mbps").unwrap(), 625_000);
        assert_eq!(parse_rate("1kb").unwrap(), 1000);
        assert_eq!(parse_rate("8kbit").unwrap(), 1000);
        assert!(parse_rate("999b").is_err());
    }

    #[test]
    fn split_and_compose_round_trip() {
        assert_eq!(split("40mbit"), Some(("40".into(), RateUnit::Mbit)));
        assert_eq!(split("5MB"), Some(("5".into(), RateUnit::MB)));
        assert_eq!(split("abc"), None);
        assert_eq!(compose("40", RateUnit::Mbit), "40mbit");
        assert_eq!(parse_rate(&compose("5", RateUnit::MB)).unwrap(), 5_000_000);
        assert_eq!(equivalent(5_000_000), "40 Mbit/s ≈ 5 MB/s");
    }
}
