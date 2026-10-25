mod file;

pub use file::LogFile;

use std::fmt::Write as _;
use std::net::IpAddr;
use std::time::SystemTime;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::{SecureRandom, SystemRandom};

use crate::LogFormat;

pub const MAX_KEPT_ID: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub time: SystemTime,
    pub client: IpAddr,
    pub host: String,
    pub method: String,
    pub target: String,
    pub protocol: &'static str,
    pub status: u16,
    pub bytes: u64,
    pub ms: u64,
    pub service: String,
    pub request_id: Option<String>,
}

pub fn new_request_id() -> String {
    let mut bytes = [0u8; 12];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        return String::from("-");
    }
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn kept_request_id(given: &str) -> bool {
    !given.is_empty()
        && given.len() <= MAX_KEPT_ID
        && given
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

struct Parts<'a> {
    day: &'a str,
    month: &'a str,
    year: &'a str,
    clock: &'a str,
}

fn parts(http_date: &str) -> Option<Parts<'_>> {
    let mut it = http_date.split(' ').skip(1);
    Some(Parts {
        day: it.next()?,
        month: it.next()?,
        year: it.next()?,
        clock: it.next()?,
    })
}

pub fn clf_time(t: SystemTime) -> String {
    let date = httpdate::fmt_http_date(t);
    parts(&date).map_or(date.clone(), |p| {
        format!("{}/{}/{}:{} +0000", p.day, p.month, p.year, p.clock)
    })
}

pub fn rfc3339_time(t: SystemTime) -> String {
    let date = httpdate::fmt_http_date(t);
    parts(&date)
        .and_then(|p| {
            let m = MONTHS.iter().position(|m| *m == p.month)? + 1;
            Some(format!("{}-{m:02}-{}T{}Z", p.year, p.day, p.clock))
        })
        .unwrap_or(date)
}

fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b == b'"' || b == b'\\' || !(0x20..0x7F).contains(&b) {
            let _ = write!(out, "\\x{b:02X}");
        } else {
            out.push(char::from(b));
        }
    }
    out
}

pub fn line(e: &Entry, format: LogFormat) -> String {
    match format {
        LogFormat::Common => format!(
            "{} - - [{}] \"{} {} {}\" {} {} {} {} {} {}",
            e.client,
            clf_time(e.time),
            escaped(&e.method),
            escaped(&e.target),
            e.protocol,
            e.status,
            e.bytes,
            escaped(&e.host),
            escaped(&e.service),
            e.ms,
            e.request_id.as_deref().map_or("-".into(), escaped),
        ),
        LogFormat::Json => serde_json::json!({
            "time": rfc3339_time(e.time),
            "client": e.client.to_string(),
            "host": e.host,
            "method": e.method,
            "path": e.target,
            "protocol": e.protocol,
            "status": e.status,
            "bytes": e.bytes,
            "ms": e.ms,
            "service": e.service,
            "request_id": e.request_id,
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests;
