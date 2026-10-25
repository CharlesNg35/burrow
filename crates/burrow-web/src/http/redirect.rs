use bytes::Bytes;
use http::{HeaderValue, Response, StatusCode, header};

use super::{Body, full};

pub fn rest_after<'a>(prefix: &str, path: &'a str) -> &'a str {
    if prefix == "/" {
        return path;
    }
    path.strip_prefix(prefix).unwrap_or(path)
}

pub fn app_path(prefix: &str, app_path: &str, path: &str) -> String {
    let joined = format!(
        "{}{}",
        app_path.trim_end_matches('/'),
        rest_after(prefix, path)
    );
    if joined.is_empty() {
        "/".into()
    } else {
        joined
    }
}

fn ascii_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for b in query.bytes() {
        if (0x21..0x7F).contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn location(prefix: &str, to: &str, path: &str, query: Option<&str>) -> String {
    let mut l = format!("{}{}", to.trim_end_matches('/'), rest_after(prefix, path));
    if l.is_empty() {
        l.push('/');
    }
    if let Some(q) = query {
        l.push('?');
        l.push_str(&ascii_query(q));
    }
    l
}

pub fn response(location: &str, permanent: bool) -> Response<Body> {
    let mut r = Response::new(full(Bytes::new()));
    *r.status_mut() = if permanent {
        StatusCode::PERMANENT_REDIRECT
    } else {
        StatusCode::TEMPORARY_REDIRECT
    };
    if let Ok(v) = HeaderValue::from_str(location) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_path_replaces_the_matched_prefix_at_its_boundary() {
        assert_eq!(app_path("/api", "/", "/api"), "/");
        assert_eq!(app_path("/api", "/", "/api/"), "/");
        assert_eq!(app_path("/api", "/", "/api/x"), "/x");
        assert_eq!(app_path("/api", "/", "/api/a%2Fb"), "/a%2Fb");
        assert_eq!(app_path("/api", "/v1", "/api"), "/v1");
        assert_eq!(app_path("/api", "/v1", "/api/x"), "/v1/x");
        assert_eq!(app_path("/api", "/v1/", "/api/x"), "/v1/x");
        assert_eq!(app_path("/shop", "/store", "/shop/cart"), "/store/cart");
        assert_eq!(app_path("/", "/v1", "/x"), "/v1/x");
    }

    #[test]
    fn a_redirect_keeps_the_rest_of_the_path_and_the_query() {
        assert_eq!(
            location("/old", "/new", "/old/page", Some("q=1")),
            "/new/page?q=1"
        );
        assert_eq!(location("/old", "/new", "/old", None), "/new");
        assert_eq!(location("/old", "/new/", "/old/", None), "/new/");
        assert_eq!(
            location("/", "https://example.com", "/a", Some("b")),
            "https://example.com/a?b"
        );
        assert_eq!(
            location("/shop", "https://shop.example.net", "/shop/cart", None),
            "https://shop.example.net/cart"
        );
        assert_eq!(
            location("/old", "https://example.com/new", "/old/x", None),
            "https://example.com/new/x"
        );
        assert_eq!(location("/old", "/", "/old", None), "/");
    }

    #[test]
    fn a_location_never_starts_protocol_relative_and_is_always_ascii() {
        let path = burrow_syntax::url_path::normalize_path("/old//evil.com").unwrap();
        let l = location("/old", "/new", &path, None);
        assert_eq!(l, "/new/evil.com");
        let path = burrow_syntax::url_path::normalize_path("//evil.com").unwrap();
        let l = location("/", "/", &path, None);
        assert!(!l.starts_with("//") && !l.starts_with("/\\"), "{l}");
        let l = location("/old", "/new", "/old/%C3%A9", Some("q=é"));
        assert_eq!(l, "/new/%C3%A9?q=%C3%A9");
        let r = response(&l, false);
        assert_eq!(r.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(r.headers()[header::LOCATION], "/new/%C3%A9?q=%C3%A9");
        assert_eq!(
            response("/x", true).status(),
            StatusCode::PERMANENT_REDIRECT
        );
    }
}
