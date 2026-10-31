#![no_main]

use std::time::{Duration, SystemTime};

use burrow_web::tls::acme::http::{MAX_RETRY_AFTER, clamp_retry_after};
use burrow_web::tls::acme::{challenge, problem_text, storage};
use burrow_web::tls::x509;
use http::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use libfuzzer_sys::fuzz_target;

fn retry_after(data: &[u8]) {
    let Ok(value) = HeaderValue::from_bytes(data) else {
        return;
    };
    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, value);
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    match clamp_retry_after(&mut headers, now) {
        Some(wait) => {
            assert!(wait <= MAX_RETRY_AFTER);
            let left: u64 = headers[RETRY_AFTER].to_str().unwrap().parse().unwrap();
            assert_eq!(
                left,
                wait.as_secs(),
                "instant-acme reads the clamped seconds"
            );
            assert!(
                SystemTime::now()
                    .checked_add(Duration::from_secs(left))
                    .is_some()
            );
        }
        None => assert!(headers.get(RETRY_AFTER).is_none()),
    }
}

fn certificate(data: &[u8]) {
    if let Some(f) = x509::facts(data)
        && let Some(id) = f.ari_id
    {
        let (aki, serial) = id.split_once('.').expect("one dot");
        for part in [aki, serial] {
            assert!(
                part.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            );
        }
    }
}

fn stored(data: &[u8]) {
    if let Ok(meta) = serde_json::from_slice::<storage::Meta>(data) {
        let back = serde_json::to_vec(&meta).unwrap();
        assert_eq!(
            serde_json::from_slice::<storage::Meta>(&back).unwrap(),
            meta
        );
    }
    if let Ok(p) = serde_json::from_slice::<instant_acme::Problem>(data) {
        let _ = problem_text(&p);
    }
}

fn names(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    if challenge::valid_token(&text) {
        assert!(!text.is_empty() && !text.contains(['/', '.']));
    }
    if let Ok(ip) = text.parse::<std::net::IpAddr>() {
        let name = challenge::reverse_name(ip);
        assert!(name.ends_with(".in-addr.arpa") || name.ends_with(".ip6.arpa"));
    }
    let _ = storage::cert_dir(std::path::Path::new("/state"), &[text.to_string()])
        .strip_prefix("/state/certs")
        .expect("inside the state folder");
}

fuzz_target!(|data: &[u8]| {
    let Some((&pick, rest)) = data.split_first() else {
        return;
    };
    match pick % 4 {
        0 => retry_after(rest),
        1 => certificate(rest),
        2 => stored(rest),
        _ => names(rest),
    }
});
