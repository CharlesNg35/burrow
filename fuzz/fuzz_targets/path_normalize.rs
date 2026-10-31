#![no_main]

use burrow_syntax::url_path::{match_view, normalize_path};
use libfuzzer_sys::fuzz_target;

fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn no_dot_segment(path: &str, splits: &[&str]) -> bool {
    let mut parts = vec![path.to_string()];
    for sep in splits {
        parts = parts
            .iter()
            .flat_map(|p| p.split(sep).map(str::to_string).collect::<Vec<_>>())
            .collect();
    }
    parts.iter().all(|s| s != "." && s != "..")
}

fn check_escapes(out: &str) {
    let b = out.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let pair = &b[i + 1..i + 3];
            assert!(
                pair.iter()
                    .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(c))
            );
            let v = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
            assert!(v != 0 && !unreserved(v), "{out:?}");
            i += 3;
        } else {
            i += 1;
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(out) = normalize_path(raw) else {
        return;
    };
    assert!(out.starts_with('/'));
    assert!(out.is_ascii());
    assert!(!out.contains("//") && !out.contains('\\'), "{out:?}");
    assert!(no_dot_segment(&out, &["/"]), "{out:?}");
    assert!(no_dot_segment(&out, &["/", "%2F", "%5C"]), "{out:?}");
    check_escapes(&out);
    assert_eq!(
        normalize_path(&out).as_deref(),
        Ok(out.as_str()),
        "idempotent"
    );
    let view = match_view(&out);
    assert!(!view.contains("//"), "{view:?}");
    assert!(no_dot_segment(&view, &["/"]), "{view:?}");
    assert_eq!(match_view(&view), view);
    assert!(
        http::uri::PathAndQuery::try_from(out.as_str()).is_ok(),
        "{out:?}"
    );
});
