use http::header::{self, HeaderMap, HeaderValue};

use crate::HeaderRule;

pub const HSTS: &str = "max-age=31536000";

pub(crate) fn has_token(h: &HeaderMap, name: header::HeaderName, token: &str) -> bool {
    h.get_all(name).iter().any(|v| {
        v.to_str()
            .is_ok_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    })
}

pub fn apply(rules: &[HeaderRule], h: &mut HeaderMap) {
    for rule in rules {
        match &rule.value {
            Some(v) => {
                h.insert(rule.name.clone(), v.clone());
            }
            None => {
                h.remove(&rule.name);
            }
        }
    }
}

pub fn add_hsts(h: &mut HeaderMap, https: bool) {
    if https {
        h.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(HSTS),
        );
    }
}

pub fn detach(h: &mut HeaderMap) {
    for v in h.values_mut() {
        if let Ok(mut own) = HeaderValue::from_bytes(v.as_bytes()) {
            own.set_sensitive(v.is_sensitive());
            *v = own;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &'static str, value: &'static str) -> HeaderRule {
        HeaderRule {
            name: http::HeaderName::from_static(name),
            value: (!value.is_empty()).then(|| HeaderValue::from_static(value)),
        }
    }

    #[test]
    fn a_value_sets_a_header_and_an_empty_value_removes_it() {
        let rules = [
            rule("host", "internal.lan"),
            rule("x-frame-options", "DENY"),
            rule("server", ""),
        ];
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "blog.example.com".parse().unwrap());
        h.insert(header::SERVER, "app/1.0".parse().unwrap());
        h.append("x-frame-options", "SAMEORIGIN".parse().unwrap());
        h.append("x-frame-options", "ALLOW".parse().unwrap());
        apply(&rules, &mut h);
        assert_eq!(h[header::HOST], "internal.lan");
        assert_eq!(h.get_all("x-frame-options").iter().count(), 1);
        assert_eq!(h["x-frame-options"], "DENY");
        assert!(!h.contains_key(header::SERVER));
    }

    #[test]
    fn hsts_is_only_sent_over_https() {
        let mut h = HeaderMap::new();
        add_hsts(&mut h, false);
        assert!(h.is_empty());
        add_hsts(&mut h, true);
        assert_eq!(h[header::STRICT_TRANSPORT_SECURITY], "max-age=31536000");
    }

    #[test]
    fn detached_values_no_longer_share_the_buffer_they_were_read_from() {
        let read = bytes::Bytes::from(vec![b'a'; 32 * 1024]);
        let mut h = HeaderMap::new();
        let mut secret = HeaderValue::from_maybe_shared(read.slice(..10)).unwrap();
        secret.set_sensitive(true);
        h.insert(
            "x-sha",
            HeaderValue::from_maybe_shared(read.slice(10..20)).unwrap(),
        );
        h.append(
            "x-sha",
            HeaderValue::from_maybe_shared(read.slice(20..30)).unwrap(),
        );
        h.insert(header::AUTHORIZATION, secret);
        let range = read.as_ptr() as usize..read.as_ptr() as usize + read.len();
        detach(&mut h);
        assert_eq!(h.get_all("x-sha").iter().count(), 2);
        for v in h.values() {
            assert_eq!(v.as_bytes(), b"aaaaaaaaaa");
            assert!(!range.contains(&(v.as_bytes().as_ptr() as usize)));
        }
        assert!(h[header::AUTHORIZATION].is_sensitive());
        drop(h);
        assert!(read.is_unique());
    }
}
