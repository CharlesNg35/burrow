use super::*;
use http::HeaderValue;

fn cidrs(list: &[&str]) -> Vec<Cidr> {
    list.iter().map(|c| c.parse().unwrap()).collect()
}

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.append(*k, HeaderValue::from_str(v).unwrap());
    }
    h
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn without_trust_the_peer_is_the_client_whatever_it_sends() {
    let h = headers(&[("x-forwarded-for", "1.2.3.4")]);
    assert_eq!(client_ip(ip("198.51.100.7"), &h, &[]), ip("198.51.100.7"));
    let t = cidrs(&["10.0.0.0/8"]);
    assert_eq!(client_ip(ip("198.51.100.7"), &h, &t), ip("198.51.100.7"));
}

#[test]
fn the_chain_is_read_right_to_left_past_trusted_hops() {
    let t = cidrs(&["10.0.0.0/8"]);
    let peer = ip("10.0.0.1");
    let walk = |pairs: &[(&'static str, &str)]| client_ip(peer, &headers(pairs), &t);
    assert_eq!(
        walk(&[("x-forwarded-for", "spoof, 203.0.113.9")]),
        ip("203.0.113.9")
    );
    assert_eq!(
        walk(&[("x-forwarded-for", "203.0.113.9, 10.0.0.2")]),
        ip("203.0.113.9")
    );
    assert_eq!(
        walk(&[
            ("x-forwarded-for", "203.0.113.9"),
            ("x-forwarded-for", "10.0.0.2")
        ]),
        ip("203.0.113.9")
    );
    assert_eq!(
        walk(&[("x-forwarded-for", "203.0.113.9:4711")]),
        ip("203.0.113.9")
    );
    assert_eq!(
        walk(&[("x-forwarded-for", "[2001:db8::1]:443")]),
        ip("2001:db8::1")
    );
    assert_eq!(
        walk(&[("x-forwarded-for", "[2001:db8::1]")]),
        ip("2001:db8::1")
    );
    assert_eq!(
        walk(&[("x-forwarded-for", "::ffff:203.0.113.9")]),
        ip("203.0.113.9")
    );
    assert_eq!(
        walk(&[("x-forwarded-for", "203.0.113.9, garbage, 10.0.0.3")]),
        ip("10.0.0.3")
    );
    assert_eq!(
        walk(&[("x-forwarded-for", "10.0.0.5, 10.0.0.3")]),
        ip("10.0.0.5")
    );
    assert_eq!(walk(&[]), peer);
}

fn peer(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

#[test]
fn the_scheme_comes_from_a_trusted_proxy_or_the_connection() {
    let t = cidrs(&["127.0.0.1/32"]);
    let https = headers(&[("x-forwarded-proto", "https")]);
    assert!(!client(peer("198.51.100.7:1"), &https, &t, false).https);
    assert!(client(peer("127.0.0.1:1"), &https, &t, false).https);
    let plain = headers(&[("x-forwarded-proto", "http")]);
    assert!(!client(peer("127.0.0.1:1"), &plain, &t, true).https);
    assert!(client(peer("127.0.0.1:1"), &HeaderMap::new(), &t, true).https);
    let both = headers(&[("x-forwarded-proto", "http, https")]);
    assert!(client(peer("127.0.0.1:1"), &both, &t, false).https);
}

#[test]
fn the_forwarded_port_follows_the_host_then_the_scheme() {
    let t = cidrs(&["127.0.0.1/32"]);
    let port = |pairs: &[(&'static str, &str)], from: &str, tls: bool| {
        client(peer(from), &headers(pairs), &t, tls).port
    };
    assert_eq!(
        port(&[("host", "example.com:8443")], "198.51.100.7:1", true),
        8443
    );
    assert_eq!(
        port(&[("host", "[2001:db8::1]:8080")], "198.51.100.7:1", false),
        8080
    );
    assert_eq!(
        port(&[("host", "example.com")], "198.51.100.7:1", false),
        80
    );
    assert_eq!(
        port(&[("host", "example.com")], "198.51.100.7:1", true),
        443
    );
    assert_eq!(
        port(&[("host", "[2001:db8::1]")], "198.51.100.7:1", true),
        443
    );
    assert_eq!(
        port(
            &[("host", "example.com"), ("x-forwarded-proto", "https")],
            "127.0.0.1:1",
            false
        ),
        443
    );
    assert_eq!(
        port(
            &[("host", "example.com"), ("x-forwarded-port", "8443")],
            "127.0.0.1:1",
            false
        ),
        8443
    );
    assert_eq!(
        port(
            &[("host", "example.com"), ("x-forwarded-port", "8443")],
            "198.51.100.7:1",
            false
        ),
        80
    );
}

#[test]
fn the_outgoing_chain_appends_the_peer_only_after_a_trusted_chain() {
    let t = cidrs(&["10.0.0.0/8"]);
    let h = headers(&[("x-forwarded-for", "203.0.113.9")]);
    assert_eq!(
        client(peer("10.0.0.2:1"), &h, &t, true).forwarded_for,
        "203.0.113.9, 10.0.0.2"
    );
    assert_eq!(
        client(peer("198.51.100.7:1"), &h, &t, true).forwarded_for,
        "198.51.100.7"
    );
    assert_eq!(
        client(peer("10.0.0.2:1"), &HeaderMap::new(), &t, true).forwarded_for,
        "10.0.0.2"
    );
}

#[test]
fn underscore_variants_go_always_and_the_rest_only_from_visitors() {
    let mut h = headers(&[
        ("x_forwarded_for", "1"),
        ("x-real_ip", "1"),
        ("forwarded", "for=1.2.3.4"),
        ("x-forwarded-prefix", "/a"),
        ("x_custom", "kept"),
    ]);
    let mut trusted = h.clone();
    strip_incoming(&mut trusted, true);
    assert!(!trusted.contains_key("x_forwarded_for") && !trusted.contains_key("x-real_ip"));
    assert!(trusted.contains_key("forwarded") && trusted.contains_key("x-forwarded-prefix"));
    strip_incoming(&mut h, false);
    assert_eq!(h.len(), 1);
    assert!(h.contains_key("x_custom"));
}
