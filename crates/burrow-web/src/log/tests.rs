use std::time::{Duration, UNIX_EPOCH};

use super::*;

fn entry() -> Entry {
    Entry {
        time: UNIX_EPOCH + Duration::from_secs(1_792_936_536),
        client: "203.0.113.9".parse().unwrap(),
        host: "blog.example.com".into(),
        method: "GET".into(),
        target: "/a?q=1".into(),
        protocol: "HTTP/1.1",
        status: 200,
        bytes: 512,
        ms: 12,
        service: "blog".into(),
        request_id: Some("abc".into()),
    }
}

#[test]
fn times_are_written_in_utc_in_both_formats() {
    let t = UNIX_EPOCH + Duration::from_secs(1_792_936_536);
    assert_eq!(clf_time(t), "25/Oct/2026:13:55:36 +0000");
    assert_eq!(rfc3339_time(t), "2026-10-25T13:55:36Z");
    let jan = UNIX_EPOCH + Duration::from_secs(1_767_225_600);
    assert_eq!(rfc3339_time(jan), "2026-01-01T00:00:00Z");
}

#[test]
fn a_common_line_has_the_request_then_host_service_ms_and_id() {
    assert_eq!(
        line(&entry(), LogFormat::Common),
        "203.0.113.9 - - [25/Oct/2026:13:55:36 +0000] \"GET /a?q=1 HTTP/1.1\" 200 512 blog.example.com blog 12 abc"
    );
    let no_id = Entry {
        request_id: None,
        ..entry()
    };
    assert!(line(&no_id, LogFormat::Common).ends_with(" 12 -"));
}

#[test]
fn quotes_backslashes_and_control_bytes_stay_on_one_escaped_line() {
    let e = Entry {
        target: "/a\"b\\c?\u{1}x=é".into(),
        ..entry()
    };
    let common = line(&e, LogFormat::Common);
    assert!(
        common.contains("\"GET /a\\x22b\\x5Cc?\\x01x=\\xC3\\xA9 HTTP/1.1\""),
        "{common}"
    );
    assert!(!common.contains('\n'));
    let json = line(&e, LogFormat::Json);
    assert!(!json.contains('\n'));
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["path"], "/a\"b\\c?\u{1}x=é");
    assert_eq!(v["time"], "2026-10-25T13:55:36Z");
    assert_eq!(v["status"], 200);
    assert_eq!(v["service"], "blog");
    assert_eq!(v["request_id"], "abc");
    assert_eq!(v["client"], "203.0.113.9");
}

#[test]
fn request_ids_are_96_random_bits_and_only_tidy_ones_are_kept() {
    let a = new_request_id();
    let b = new_request_id();
    assert_eq!(a.len(), 16);
    assert_ne!(a, b);
    assert!(
        a.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    );
    assert!(kept_request_id("f3a1-77"));
    assert!(kept_request_id(&"x".repeat(MAX_KEPT_ID)));
    for bad in ["", "a b", "a\"b", "a\\b", "é", &"x".repeat(MAX_KEPT_ID + 1)] {
        assert!(!kept_request_id(bad), "{bad:?}");
    }
}
