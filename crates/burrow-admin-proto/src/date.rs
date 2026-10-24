const DAY: u64 = 86_400;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn date(unix: u64) -> String {
    let z = (unix / DAY) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unix_time_reads_as_a_day_month_and_year() {
        assert_eq!(date(0), "1 Jan 1970");
        assert_eq!(date(951_782_400), "29 Feb 2000");
        assert_eq!(date(4_102_444_800), "1 Jan 2100");
        assert_eq!(date(66_303_014_400), "22 Jan 4071");
    }
}
