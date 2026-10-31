const DAY: u64 = 86_400;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn date(unix: u64) -> String {
    let (y, m, d) = civil(unix);
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

pub fn year_month(unix: u64) -> String {
    let (y, m, _) = civil(unix);
    format!("{y:04}-{m:02}")
}

fn civil(unix: u64) -> (i64, i64, i64) {
    let z = (unix / DAY) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub fn date_time(unix: u64) -> String {
    let secs = unix % DAY;
    format!(
        "{} {:02}:{:02} UTC",
        date(unix),
        secs / 3600,
        secs % 3600 / 60
    )
}

pub fn order_line(o: &crate::status::CertOrderStatus, now: u64) -> String {
    let names = o.names.join(", ");
    let when = if o.running {
        "ordering now".to_string()
    } else if o.at <= now {
        "next in line".to_string()
    } else {
        format!("next try {}", date_time(o.at))
    };
    format!("{} ({names}): {when}", o.service)
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
        assert_eq!(date_time(4_102_444_800 + 3_723), "1 Jan 2100 01:02 UTC");
        assert_eq!(year_month(951_782_400), "2000-02");
        assert_eq!(year_month(4_102_444_800 - 1), "2099-12");
    }

    #[test]
    fn an_order_says_whether_it_runs_or_when_it_is_tried() {
        let o = crate::status::CertOrderStatus {
            service: "blog".into(),
            names: vec!["blog.example.com".into(), "www.example.com".into()],
            at: 4_102_444_800,
            running: true,
        };
        assert_eq!(
            order_line(&o, 0),
            "blog (blog.example.com, www.example.com): ordering now"
        );
        let later = crate::status::CertOrderStatus {
            running: false,
            ..o
        };
        assert_eq!(
            order_line(&later, 0),
            "blog (blog.example.com, www.example.com): next try 1 Jan 2100 00:00 UTC"
        );
        assert!(order_line(&later, 4_102_444_801).ends_with("next in line"));
    }
}
