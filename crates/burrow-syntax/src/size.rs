pub fn parse_size(text: &str) -> Result<u64, String> {
    let t = text.trim().to_ascii_lowercase();
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (number, unit) = t.split_at(split);
    let value: f64 = number
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite() && *v > 0.0)
        .ok_or_else(|| format!("\"{}\" does not start with a positive number", text.trim()))?;
    let scale = match unit.trim() {
        "" | "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        other => {
            return Err(format!(
                "\"{other}\" is not a size unit; use B, KB, MB, GB or TB, like 100MB"
            ));
        }
    };
    let bytes = (value * scale).round();
    if bytes < 1.0 || bytes >= u64::MAX as f64 {
        return Err(format!("{} is not a size burrow can use", text.trim()));
    }
    Ok(bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_bandwidth_with_decimal_and_binary_units() {
        assert_eq!(parse_size("100MB"), Ok(100_000_000));
        assert_eq!(parse_size("8GB"), Ok(8_000_000_000));
        assert_eq!(parse_size(" 512kb "), Ok(512_000));
        assert_eq!(parse_size("1.5MB"), Ok(1_500_000));
        assert_eq!(parse_size("1MiB"), Ok(1_048_576));
        assert_eq!(parse_size("2048"), Ok(2048));
        assert_eq!(parse_size("10B"), Ok(10));
    }

    #[test]
    fn a_size_without_a_positive_number_or_with_another_unit_is_refused() {
        assert!(parse_size("MB").is_err());
        assert!(parse_size("0MB").is_err());
        assert!(parse_size("-1MB").is_err());
        assert!(parse_size("10 parsecs").is_err());
        assert!(parse_size("10mbit").is_err());
        assert!(parse_size("").is_err());
    }
}
