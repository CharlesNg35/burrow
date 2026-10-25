use http::header::{self, HeaderMap, HeaderName, HeaderValue};

pub const HSTS: &str = "max-age=31536000";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules(pub Vec<(HeaderName, Option<HeaderValue>)>);

impl Rules {
    pub fn parse(pairs: &[(String, String)]) -> Result<Rules, String> {
        let mut out = Vec::with_capacity(pairs.len());
        for (name, value) in pairs {
            let n = HeaderName::from_bytes(name.trim().as_bytes())
                .map_err(|_| format!("\"{name}\" is not a header name"))?;
            let v = if value.is_empty() {
                None
            } else {
                Some(
                    HeaderValue::from_str(value)
                        .map_err(|_| format!("the value of \"{name}\" is not a header value"))?,
                )
            };
            out.push((n, v));
        }
        Ok(Rules(out))
    }

    pub fn apply(&self, h: &mut HeaderMap) {
        for (name, value) in &self.0 {
            match value {
                Some(v) => {
                    h.insert(name.clone(), v.clone());
                }
                None => {
                    h.remove(name);
                }
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

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_value_sets_a_header_and_an_empty_value_removes_it() {
        let rules = Rules::parse(&pairs(&[
            ("Host", "internal.lan"),
            ("X-Frame-Options", "DENY"),
            ("Server", ""),
        ]))
        .unwrap();
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "blog.example.com".parse().unwrap());
        h.insert(header::SERVER, "app/1.0".parse().unwrap());
        h.append("x-frame-options", "SAMEORIGIN".parse().unwrap());
        h.append("x-frame-options", "ALLOW".parse().unwrap());
        rules.apply(&mut h);
        assert_eq!(h[header::HOST], "internal.lan");
        assert_eq!(h.get_all("x-frame-options").iter().count(), 1);
        assert_eq!(h["x-frame-options"], "DENY");
        assert!(!h.contains_key(header::SERVER));
    }

    #[test]
    fn bad_names_or_values_are_refused() {
        assert!(Rules::parse(&pairs(&[("Bad Name", "x")])).is_err());
        assert!(Rules::parse(&pairs(&[("X-Ok", "a\nb")])).is_err());
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
