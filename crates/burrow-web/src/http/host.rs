use std::net::IpAddr;

use http::uri::Authority;
use http::{HeaderMap, Uri, header};

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
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Some(ip.to_canonical().to_string());
    }
    let name = bare.strip_suffix('.').unwrap_or(bare);
    (!name.is_empty() && !name.contains('[')).then(|| name.to_ascii_lowercase())
}

pub fn request_host(uri: &Uri, headers: &HeaderMap) -> Option<String> {
    let mut given = headers.get_all(header::HOST).iter();
    let first = given.next()?;
    if given.next().is_some() {
        return None;
    }
    let field = host_header(first.to_str().ok()?)?;
    match uri.authority() {
        Some(a) => host_header(a.as_str()),
        None => Some(field),
    }
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
    fn a_host_that_is_not_an_authority_is_refused() {
        for bad in [
            "",
            "user@blog.example.com",
            "a b",
            "blog.example.com:x",
            "/path",
            ".",
            "a.example.com:99999",
            "a.example.com:",
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

    #[test]
    fn a_request_needs_exactly_one_valid_host_and_its_absolute_target_wins() {
        let origin: Uri = "/x".parse().unwrap();
        let absolute: Uri = "https://Other.example.com/x".parse().unwrap();
        assert_eq!(
            request_host(&origin, &headers(&["a.example.com"])).as_deref(),
            Some("a.example.com")
        );
        assert_eq!(request_host(&origin, &headers(&[])), None);
        assert_eq!(
            request_host(&origin, &headers(&["a.example.com", "a.example.com"])),
            None
        );
        assert_eq!(request_host(&origin, &headers(&["a@b"])), None);
        assert_eq!(
            request_host(&absolute, &headers(&["a.example.com"])).as_deref(),
            Some("other.example.com")
        );
    }
}
