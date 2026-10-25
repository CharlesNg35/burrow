use std::net::IpAddr;

use http::Method;
use http::header::{HeaderMap, HeaderName, HeaderValue};

use super::*;

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (n, v) in pairs {
        h.append(
            HeaderName::from_bytes(n.as_bytes()).unwrap(),
            HeaderValue::from_str(v).unwrap(),
        );
    }
    h
}

fn names(list: &[&str]) -> Vec<HeaderName> {
    list.iter()
        .map(|n| HeaderName::from_bytes(n.as_bytes()).unwrap())
        .collect()
}

fn client() -> Client {
    Client {
        ip: "203.0.113.9".parse::<IpAddr>().unwrap(),
        trusted_peer: false,
        https: true,
        port: 443,
        forwarded_for: "203.0.113.9".into(),
    }
}

#[test]
fn visitor_copies_and_their_underscore_variants_are_removed() {
    let copy = names(&["remote-user", "remote-groups"]);
    let mut h = headers(&[
        ("remote-user", "root"),
        ("Remote_User", "root"),
        ("REMOTE-USER", "root"),
        ("remote_groups", "admins"),
        ("remote-email", "kept@example.com"),
        ("cookie", "a=b"),
    ]);
    strip_visitor_copies(&mut h, &copy);
    assert!(h.get("remote-user").is_none());
    assert!(h.get("remote_user").is_none());
    assert!(h.get("remote_groups").is_none());
    assert_eq!(h.get("remote-email").unwrap(), "kept@example.com");
    assert_eq!(h.get("cookie").unwrap(), "a=b");
}

#[test]
fn connection_tokens_go_now_so_they_cannot_remove_what_burrow_adds_later() {
    let copy = names(&["remote-user"]);
    let mut h = headers(&[
        (
            "connection",
            "Remote-User, X-Forwarded-For, upgrade, x-secret",
        ),
        ("upgrade", "websocket"),
        ("x-secret", "1"),
    ]);
    strip_visitor_copies(&mut h, &copy);
    assert_eq!(h.get("connection").unwrap(), "upgrade");
    assert_eq!(h.get("upgrade").unwrap(), "websocket");
    assert!(h.get("x-secret").is_none());
    let mut plain = headers(&[("connection", "keep-alive, remote-user")]);
    strip_visitor_copies(&mut plain, &copy);
    assert!(plain.get("connection").is_none());
}

#[test]
fn the_subrequest_is_a_bodiless_get_with_burrows_own_forwarded_values() {
    let visitor = headers(&[
        ("host", "wiki.example.com"),
        ("content-length", "12"),
        ("transfer-encoding", "chunked"),
        ("expect", "100-continue"),
        ("te", "trailers"),
        ("x-forwarded-for", "169.254.1.2"),
        ("x_forwarded_for", "169.254.1.2"),
        ("x-forwarded-uri", "/public"),
        ("x-forwarded-prefix", "/x"),
        ("x-forwarded-port", "1"),
        ("forwarded", "for=1.2.3.4"),
        ("x-original-url", "https://wiki.example.com/public"),
        ("X_Original_URL", "https://wiki.example.com/public"),
        ("x-original-method", "GET"),
        ("x-original-uri", "/public"),
        ("cookie", "authelia_session=abc"),
        ("authorization", "Basic eDp5"),
    ]);
    let method = Method::POST;
    let asked = Asked {
        method: &method,
        host: "wiki.example.com",
        path_and_query: "/admin/x?y=1",
        client: &client(),
    };
    let req = subrequest("/api/authz/forward-auth", &visitor, &asked).unwrap();
    assert_eq!(req.method(), Method::GET);
    assert_eq!(req.uri(), "/api/authz/forward-auth");
    let h = req.headers();
    for gone in [
        "content-length",
        "transfer-encoding",
        "expect",
        "te",
        "x_forwarded_for",
        "x-forwarded-prefix",
        "x-forwarded-port",
        "forwarded",
        "x_original_url",
    ] {
        assert!(h.get(gone).is_none(), "{gone}");
    }
    let one = |n: &str| {
        let all: Vec<_> = h.get_all(n).iter().collect();
        assert_eq!(all.len(), 1, "{n}");
        all[0].to_str().unwrap().to_string()
    };
    assert_eq!(one("x-forwarded-for"), "203.0.113.9");
    assert_eq!(one("x-forwarded-method"), "POST");
    assert_eq!(one("x-forwarded-proto"), "https");
    assert_eq!(one("x-forwarded-host"), "wiki.example.com");
    assert_eq!(one("x-forwarded-uri"), "/admin/x?y=1");
    assert_eq!(
        one("x-original-url"),
        "https://wiki.example.com/admin/x?y=1"
    );
    assert_eq!(one("x-original-uri"), "/admin/x?y=1");
    assert_eq!(one("x-original-method"), "POST");
    assert_eq!(one("host"), "wiki.example.com");
    assert_eq!(one("cookie"), "authelia_session=abc");
    assert_eq!(one("authorization"), "Basic eDp5");
}

#[test]
fn a_grant_copies_every_value_of_each_listed_header_and_nothing_else() {
    let copy = names(&["remote-user", "remote-groups"]);
    let auth = headers(&[
        ("remote-user", "alice"),
        ("remote-groups", "dev"),
        ("remote-groups", "ops"),
        ("set-cookie", "x=y"),
    ]);
    let mut to = headers(&[("remote-user", "stale")]);
    copy_granted(&auth, &mut to, &copy);
    assert_eq!(to.get("remote-user").unwrap(), "alice");
    let groups: Vec<_> = to.get_all("remote-groups").iter().collect();
    assert_eq!(groups, ["dev", "ops"]);
    assert!(to.get("set-cookie").is_none());
}
