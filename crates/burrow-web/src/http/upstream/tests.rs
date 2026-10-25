use super::*;

fn visitor(https: bool) -> Client {
    Client {
        ip: "198.51.100.7".parse().unwrap(),
        trusted_peer: false,
        https,
        port: if https { 443 } else { 80 },
        forwarded_for: "198.51.100.7".into(),
    }
}

fn request(target: &str, headers: &[(&str, &str)]) -> Request<()> {
    let mut b = Request::get(target);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    b.body(()).unwrap()
}

#[test]
fn hop_by_hop_headers_and_those_named_in_connection_are_removed() {
    let mut req = request(
        "/x?y=1",
        &[
            ("host", "blog.example.com"),
            ("connection", "keep-alive, X-Secret"),
            ("x-secret", "1"),
            ("keep-alive", "timeout=5"),
            ("proxy-connection", "keep-alive"),
            ("te", "trailers"),
            ("trailer", "x-t"),
            ("upgrade", "h2c"),
            ("accept", "text/html"),
        ],
    );
    rewrite_request(&mut req, "blog.example.com", &visitor(true));
    let h = req.headers();
    for gone in [
        "connection",
        "x-secret",
        "keep-alive",
        "proxy-connection",
        "te",
        "trailer",
        "upgrade",
    ] {
        assert!(!h.contains_key(gone), "{gone}");
    }
    assert_eq!(h["accept"], "text/html");
    assert_eq!(h["host"], "blog.example.com");
    assert_eq!(req.uri(), "/x?y=1");
}

#[test]
fn the_forwarded_headers_name_the_visitor_and_replace_what_it_sent() {
    let mut req = request(
        "/",
        &[
            ("host", "blog.example.com"),
            ("x-forwarded-for", "6.6.6.6"),
            ("x-real-ip", "6.6.6.6"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-host", "evil.example.com"),
            ("x-forwarded-port", "1"),
        ],
    );
    rewrite_request(&mut req, "blog.example.com", &visitor(false));
    let h = req.headers();
    assert_eq!(h.get_all("x-forwarded-for").iter().count(), 1);
    assert_eq!(h["x-forwarded-for"], "198.51.100.7");
    assert_eq!(h["x-real-ip"], "198.51.100.7");
    assert_eq!(h["x-forwarded-proto"], "http");
    assert_eq!(h["x-forwarded-host"], "blog.example.com");
    assert_eq!(h["x-forwarded-port"], "80");
}

#[test]
fn an_upgrade_is_kept_and_an_absolute_target_becomes_origin_form() {
    let mut req = request(
        "https://Blog.example.com/chat?room=1",
        &[
            ("host", "blog.example.com"),
            ("connection", "Upgrade"),
            ("upgrade", "websocket"),
        ],
    );
    rewrite_request(&mut req, "blog.example.com", &visitor(true));
    assert_eq!(req.uri(), "/chat?room=1");
    let h = req.headers();
    assert_eq!(h["connection"], "upgrade");
    assert_eq!(h["upgrade"], "websocket");
    assert_eq!(h["host"], "blog.example.com");
    let mut plain = request("/", &[("upgrade", "websocket")]);
    assert!(
        upgrade_of(plain.headers()).is_none(),
        "Upgrade without Connection: upgrade is not an upgrade"
    );
    rewrite_request(&mut plain, "blog.example.com", &visitor(true));
    assert!(!plain.headers().contains_key("upgrade"));
}

#[test]
fn each_refusal_before_the_app_answers_has_its_page() {
    assert_eq!(refusal_page(Refused::NotAllowed), Page::Forbidden);
    assert_eq!(refusal_page(Refused::Full), Page::Busy);
    assert_eq!(refusal_page(Refused::NoClient), Page::Unavailable);
    assert_eq!(refusal_page(Refused::Gone), Page::NotFound);
}

#[test]
fn connection_tokens_never_remove_the_host_or_the_forwarded_headers() {
    let mut req = request(
        "/",
        &[
            ("host", "blog.example.com"),
            (
                "connection",
                "close, host, X-Forwarded-For, X-Real-IP, X-Forwarded-Proto, X-Forwarded-Port, X-Forwarded-Host, X-Request-Id, X-Forwarded-Prefix, x-other",
            ),
            ("x-other", "1"),
        ],
    );
    rewrite_request(&mut req, "blog.example.com", &visitor(true));
    let h = req.headers();
    assert_eq!(h["host"], "blog.example.com");
    assert_eq!(h["x-forwarded-for"], "198.51.100.7");
    assert_eq!(h["x-real-ip"], "198.51.100.7");
    assert_eq!(h["x-forwarded-proto"], "https");
    assert_eq!(h["x-forwarded-port"], "443");
    assert_eq!(h["x-forwarded-host"], "blog.example.com");
    assert!(!h.contains_key("x-other"));
    assert!(!h.contains_key("connection"));
}

#[test]
fn a_visitor_cannot_send_forwarded_headers_of_its_own() {
    let sent = [
        ("forwarded", "for=1.2.3.4;proto=https"),
        ("x-forwarded-prefix", "/admin"),
        ("x-forwarded-uri", "/x"),
        ("x-forwarded-method", "GET"),
        ("x-forwarded-server", "evil"),
        ("x-forwarded-ssl", "on"),
        ("x-forwarded-scheme", "https"),
        ("x-scheme", "https"),
        ("x_forwarded_for", "1.2.3.4"),
        ("x_real_ip", "1.2.3.4"),
        ("x-forwarded_host", "evil"),
    ];
    let mut req = request("/", &sent);
    rewrite_request(&mut req, "blog.example.com", &visitor(false));
    for (name, _) in sent {
        assert!(!req.headers().contains_key(name), "{name}");
    }
    assert_eq!(req.headers()["x-forwarded-proto"], "http");
}

#[test]
fn a_trusted_proxy_keeps_its_headers_and_the_chain_grows() {
    let mut req = request(
        "/",
        &[
            ("host", "blog.example.com"),
            ("x-forwarded-for", "203.0.113.9"),
            ("x-forwarded-host", "public.example.com"),
            ("x-forwarded-prefix", "/app"),
            ("x_forwarded_for", "6.6.6.6"),
        ],
    );
    let client = Client {
        ip: "203.0.113.9".parse().unwrap(),
        trusted_peer: true,
        https: true,
        port: 443,
        forwarded_for: "203.0.113.9, 10.0.0.2".into(),
    };
    rewrite_request(&mut req, "blog.example.com", &client);
    let h = req.headers();
    assert_eq!(h["x-forwarded-for"], "203.0.113.9, 10.0.0.2");
    assert_eq!(h["x-real-ip"], "203.0.113.9");
    assert_eq!(h["x-forwarded-host"], "public.example.com");
    assert_eq!(h["x-forwarded-prefix"], "/app");
    assert!(!h.contains_key("x_forwarded_for"));
}

#[test]
fn only_a_websocket_upgrade_reaches_the_app() {
    let mut h2c = request(
        "/",
        &[
            ("host", "blog.example.com"),
            ("connection", "Upgrade, HTTP2-Settings"),
            ("upgrade", "h2c"),
            ("http2-settings", "AAMAAABkAARAAAAAAAIAAAAA"),
        ],
    );
    assert!(upgrade_of(h2c.headers()).is_none());
    rewrite_request(&mut h2c, "blog.example.com", &visitor(true));
    let h = h2c.headers();
    assert!(!h.contains_key("upgrade") && !h.contains_key("connection"));
    assert!(!h.contains_key("http2-settings"));
    let ws = request(
        "/",
        &[
            ("connection", "keep-alive, Upgrade"),
            ("upgrade", "WebSocket"),
        ],
    );
    assert_eq!(upgrade_of(ws.headers()).unwrap(), "websocket");
    let both = request(
        "/",
        &[("connection", "upgrade"), ("upgrade", "h2c, websocket")],
    );
    assert_eq!(upgrade_of(both.headers()).unwrap(), "websocket");
}
