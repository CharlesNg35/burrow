#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Unit {
    bits: bool,
    power: i32,
}

impl Unit {
    fn bytes(self) -> f64 {
        let scale = 10f64.powi(self.power);
        if self.bits { scale / 8.0 } else { scale }
    }
}

fn unit_of(text: &str) -> Option<Unit> {
    let t = text.trim();
    let t = t.strip_suffix("/s").unwrap_or(t).trim_end();
    let lower = t.to_ascii_lowercase();
    let (bits, prefix) = match t.strip_suffix("Bps") {
        Some(p) => (false, p.to_ascii_lowercase()),
        None => match lower
            .strip_suffix("bps")
            .or_else(|| lower.strip_suffix("bit"))
        {
            Some(p) => (true, p.to_string()),
            None => (false, lower.strip_suffix('b')?.to_string()),
        },
    };
    let power = match prefix.as_str() {
        "" => 0,
        "k" => 3,
        "m" => 6,
        "g" => 9,
        _ => return None,
    };
    Some(Unit { bits, power })
}

fn number_and_unit(text: &str) -> (&str, &str) {
    let t = text.trim();
    let at = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    t.split_at(at)
}

pub fn parse_rate(text: &str) -> Result<u64, String> {
    let (number, unit) = number_and_unit(text);
    let value: f64 = number
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite() && *v > 0.0)
        .ok_or_else(|| format!("\"{text}\" does not start with a positive number"))?;
    let unit = unit_of(unit).ok_or_else(|| format!("\"{}\" is not a rate unit", unit.trim()))?;
    let bytes = value * unit.bytes();
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

    fn unit(self) -> Unit {
        let (bits, power) = match self {
            RateUnit::Kbit => (true, 3),
            RateUnit::Mbit => (true, 6),
            RateUnit::Gbit => (true, 9),
            RateUnit::KB => (false, 3),
            RateUnit::MB => (false, 6),
            RateUnit::GB => (false, 9),
        };
        Unit { bits, power }
    }

    pub fn from_suffix(s: &str) -> Option<RateUnit> {
        RateUnit::ALL.into_iter().find(|u| u.suffix() == s)
    }
}

pub fn split(text: &str) -> Option<(String, RateUnit)> {
    let (n, u) = number_and_unit(text);
    if n.is_empty() {
        return None;
    }
    let unit = unit_of(u)?;
    let unit = RateUnit::ALL.into_iter().find(|r| r.unit() == unit)?;
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

    #[test]
    fn split_agrees_with_parse_rate() {
        for x in [
            "5MBps",
            "5Mbps",
            "5mbps",
            "5KBps",
            "50Kbps",
            "40mbit",
            "40Mbit/s",
            "5MB",
            "2.5gbit",
            "3GBps",
            "7kb",
            "70 kbit/s",
        ] {
            let (n, u) = split(x).unwrap();
            assert_eq!(
                parse_rate(&compose(&n, u)).ok(),
                Some(parse_rate(x).unwrap()),
                "{x}"
            );
        }
        assert_eq!(split("5MBps"), Some(("5".into(), RateUnit::MB)));
        for u in RateUnit::ALL {
            assert_eq!(split(&compose("9", u)), Some(("9".into(), u)));
        }
    }
}
