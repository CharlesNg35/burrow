use http::header::{self, HeaderMap, HeaderValue};

use crate::HeaderRule;

pub const HSTS: &str = "max-age=31536000";

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
}
