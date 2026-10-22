pub fn span(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, _) => format!("{m}m"),
        (0, _, _) => format!("{h}h {m}m"),
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

pub fn left(left_secs_ago: Option<u64>, uptime: u64) -> String {
    match left_secs_ago {
        Some(a) => format!("left {}", ago(a)),
        None => format!("not seen since server start ({})", span(uptime)),
    }
}

pub fn visitor_use(open: u64, last_seen_secs: Option<u64>) -> String {
    match (open, last_seen_secs) {
        (1.., _) => format!("in use now · {open} open"),
        (0, Some(s)) => format!("last used {}", ago(s)),
        (0, None) => "not seen since the server started".into(),
    }
}

pub fn visitor_use_short(open: u64, last_seen_secs: Option<u64>) -> String {
    match (open, last_seen_secs) {
        (1.., _) => format!("{open} open"),
        (0, Some(s)) => ago(s),
        (0, None) => "not seen".into(),
    }
}

pub fn away_badge(left_secs_ago: Option<u64>) -> &'static str {
    if left_secs_ago.is_some() {
        "away"
    } else {
        "not seen"
    }
}

pub fn listen_addr(label: &str) -> String {
    label.rsplit(' ').next().unwrap_or(label).to_string()
}

pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn bytes(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i + 1 < units.len() {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", units[i])
    }
}

pub fn relative(unix: u64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(unix as f64 * 1000.0));
    let now = js_sys::Date::new_0();
    let day = |x: &js_sys::Date| (x.get_full_year(), x.get_month(), x.get_date());
    let yesterday = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
        now.get_time() - 86_400_000.0,
    ));
    let hm = format!("{:02}:{:02}", d.get_hours(), d.get_minutes());
    if day(&d) == day(&now) {
        format!("today {hm}")
    } else if day(&d) == day(&yesterday) {
        format!("yday {hm}")
    } else {
        let months = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        format!(
            "{} {:02} {hm}",
            months[d.get_month() as usize % 12],
            d.get_date()
        )
    }
}

pub fn clock(ms: f64) -> String {
    if ms <= 0.0 {
        return "--:--:--".into();
    }
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    format!(
        "{:02}:{:02}:{:02}",
        d.get_hours(),
        d.get_minutes(),
        d.get_seconds()
    )
}
