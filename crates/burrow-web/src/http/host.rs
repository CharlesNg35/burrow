use std::net::{Ipv4Addr, Ipv6Addr};

use http::uri::Authority;
use http::{HeaderMap, Uri, Version, header};

pub fn host_header(value: &str) -> Option<String> {
    if value.contains('@') {
        return None;
    }
    let authority: Authority = value.parse().ok()?;
    let host = authority.host();
    let port = &authority.as_str()[host.len()..];
    if !port.is_empty()
        && port
            .strip_prefix(':')
            .is_none_or(|p| p.parse::<u16>().is_err())
    {
        return None;
    }
    if let Some(literal) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        let ip = literal.parse::<Ipv6Addr>().ok()?;
        return Some(ip.to_canonical().to_string());
    }
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Some(ip.to_string());
    }
    let name = host.strip_suffix('.').unwrap_or(host);
    let labels_ok = !name.is_empty() && name.split('.').all(|l| !l.is_empty());
    (labels_ok && !name.contains(['[', ']'])).then(|| name.to_ascii_lowercase())
}

pub fn request_host(uri: &Uri, headers: &HeaderMap, version: Version) -> Option<String> {
    let mut given = headers.get_all(header::HOST).iter();
    let first = given.next();
    if given.next().is_some() {
        return None;
    }
    let field = match first {
        Some(v) => Some(host_header(v.to_str().ok()?)?),
        None => None,
    };
    let authority = match uri.authority() {
        Some(a) => Some(host_header(a.as_str())?),
        None => None,
    };
    if version == Version::HTTP_2 {
        return match (authority, field) {
            (Some(a), Some(f)) if a != f => None,
            (Some(a), _) => Some(a),
            (None, f) => f,
        };
    }
    field.as_ref()?;
    authority.or(field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn a_host_loses_its_port_and_final_dot_and_is_lower_case() {
        assert_eq!(
            host_header("Blog.Example.com:8443").as_deref(),
            Some("blog.example.com")
        );
        assert_eq!(
            host_header("blog.example.com.").as_deref(),
            Some("blog.example.com")
        );
        assert_eq!(host_header("nas").as_deref(), Some("nas"));
        assert_eq!(
            host_header("[2001:DB8::1]:443").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(
            host_header("[::ffff:203.0.113.10]").as_deref(),
            Some("203.0.113.10")
        );
        assert_eq!(
            host_header("203.0.113.10:80").as_deref(),
            Some("203.0.113.10")
        );
    }

    #[test]
    fn a_host_that_is_not_an_authority_or_a_dns_shaped_name_is_refused() {
        for bad in [
            "",
            "user@blog.example.com",
            "a b",
            "blog.example.com:x",
            "/path",
            ".",
            "a.example.com:99999",
            "a.example.com:",
            "a..example.com",
            "blog.example.com..",
            ".blog.example.com",
            "[blog.example.com]",
            "[v1.x]",
        ] {
            assert_eq!(host_header(bad), None, "{bad:?}");
        }
    }

    fn headers(hosts: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in hosts {
            h.append(header::HOST, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    const H1: Version = Version::HTTP_11;

    #[test]
    fn a_request_needs_exactly_one_valid_host_and_its_absolute_target_wins() {
        let origin: Uri = "/x".parse().unwrap();
        let absolute: Uri = "https://Other.example.com/x".parse().unwrap();
        assert_eq!(
            request_host(&origin, &headers(&["a.example.com"]), H1).as_deref(),
            Some("a.example.com")
        );
        assert_eq!(request_host(&origin, &headers(&[]), H1), None);
        assert_eq!(
            request_host(&origin, &headers(&["a.example.com", "a.example.com"]), H1),
            None
        );
        assert_eq!(request_host(&origin, &headers(&["a@b"]), H1), None);
        assert_eq!(
            request_host(&absolute, &headers(&["a.example.com"]), H1).as_deref(),
            Some("other.example.com")
        );
    }

    #[test]
    fn on_http2_the_authority_names_the_host_and_a_different_host_field_is_refused() {
        let h2 = Version::HTTP_2;
        let authority: Uri = "https://Blog.example.com:443/x".parse().unwrap();
        let origin: Uri = "/x".parse().unwrap();
        assert_eq!(
            request_host(&authority, &headers(&[]), h2).as_deref(),
            Some("blog.example.com"),
            ":authority alone"
        );
        assert_eq!(
            request_host(&authority, &headers(&["blog.example.com"]), h2).as_deref(),
            Some("blog.example.com")
        );
        assert_eq!(
            request_host(&authority, &headers(&["other.example.com"]), h2),
            None,
            "RFC 9113 8.3.1: Host and :authority must agree"
        );
        assert_eq!(
            request_host(&origin, &headers(&["blog.example.com"]), h2).as_deref(),
            Some("blog.example.com"),
            "Host alone"
        );
        assert_eq!(request_host(&origin, &headers(&[]), h2), None);
        assert_eq!(request_host(&authority, &headers(&["a", "a"]), h2), None);
    }
}
