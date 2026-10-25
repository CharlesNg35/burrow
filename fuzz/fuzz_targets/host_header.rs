#![no_main]

use burrow_syntax::web::{SingleLabel, host_name, is_wildcard};
use burrow_web::http::host::{host_header, request_host};
use http::{HeaderMap, HeaderValue, Uri, Version, header};
use libfuzzer_sys::fuzz_target;

fn written(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn normal(text: &str) {
    let Some(h) = host_header(text) else {
        assert_eq!(host_header(&text.to_ascii_uppercase()), None, "{text:?}");
        return;
    };
    assert!(!h.is_empty());
    assert_eq!(h, h.to_ascii_lowercase(), "{text:?}");
    assert!(
        !h.contains(['@', '[', ']', '/', ' ']),
        "{text:?} gave {h:?}"
    );
    assert!(
        !h.contains(':') || h.parse::<std::net::Ipv6Addr>().is_ok(),
        "{text:?} gave {h:?}"
    );
    assert_eq!(
        host_header(&written(&h)),
        Some(h.clone()),
        "{text:?} gave {h:?}"
    );
    assert_eq!(
        host_header(&format!("{}:8443", written(&h))),
        Some(h.clone())
    );
    assert_eq!(
        host_header(&text.to_ascii_uppercase()),
        Some(h.clone()),
        "{text:?}"
    );
}

fn configured(text: &str) {
    let Ok(name) = host_name(text, SingleLabel::Allowed) else {
        return;
    };
    if is_wildcard(&name) {
        return;
    }
    let shown = written(&name);
    assert_eq!(
        host_header(&shown),
        Some(name.clone()),
        "a configured name is reachable"
    );
    assert_eq!(
        host_header(&format!("{}:80", shown.to_ascii_uppercase())),
        Some(name.clone())
    );
    if !name.contains(':') {
        assert_eq!(host_header(&format!("{name}.")), Some(name));
    }
}

fn request(data: &[u8], h2: bool) {
    let mut parts = data.split(|b| *b == b'\n');
    let target = parts.next().unwrap_or_default();
    let Ok(uri) = Uri::try_from(target) else {
        return;
    };
    let mut headers = HeaderMap::new();
    let mut values = Vec::new();
    for v in parts.take(4) {
        if let Ok(v) = HeaderValue::from_bytes(v) {
            headers.append(header::HOST, v.clone());
            values.push(v);
        }
    }
    let version = if h2 {
        Version::HTTP_2
    } else {
        Version::HTTP_11
    };
    let got = request_host(&uri, &headers, version);
    let field = match values.as_slice() {
        [one] => Some(one.to_str().ok().and_then(host_header)),
        [] => None,
        _ => Some(None),
    };
    let authority = uri.authority().map(|a| host_header(a.as_str()));
    let want = match (field, authority) {
        (Some(None), _) | (_, Some(None)) => None,
        (Some(Some(f)), Some(Some(a))) if h2 && f != a => None,
        (_, Some(Some(a))) if h2 => Some(a),
        (f, None) if h2 => f.flatten(),
        (None, _) => None,
        (Some(Some(_)), Some(Some(a))) => Some(a),
        (Some(Some(f)), None) => Some(f),
    };
    assert_eq!(got, want, "{uri} {values:?}");
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    if mode % 2 == 1 {
        request(rest, mode & 2 == 2);
        return;
    }
    if let Ok(text) = std::str::from_utf8(rest) {
        normal(text);
        configured(text);
    }
});
