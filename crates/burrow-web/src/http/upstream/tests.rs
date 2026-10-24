use super::*;

fn visitor() -> Visitor {
    Visitor {
        addr: "[::ffff:198.51.100.7]:50000".parse().unwrap(),
        local: "[::]:8443".parse().unwrap(),
        host: "blog.example.com".into(),
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
    rewrite_request(&mut req, "blog.example.com", &visitor(), true);
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
    rewrite_request(&mut req, "blog.example.com", &visitor(), false);
    let h = req.headers();
    assert_eq!(h.get_all("x-forwarded-for").iter().count(), 1);
    assert_eq!(h["x-forwarded-for"], "198.51.100.7");
    assert_eq!(h["x-real-ip"], "198.51.100.7");
    assert_eq!(h["x-forwarded-proto"], "http");
    assert_eq!(h["x-forwarded-host"], "blog.example.com");
    assert_eq!(h["x-forwarded-port"], "8443");
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
    rewrite_request(&mut req, "blog.example.com", &visitor(), true);
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
    rewrite_request(&mut plain, "blog.example.com", &visitor(), true);
    assert!(!plain.headers().contains_key("upgrade"));
}

#[test]
fn each_refusal_before_the_app_answers_has_its_page() {
    assert_eq!(refusal_page(Refused::NotAllowed), Page::Forbidden);
    assert_eq!(refusal_page(Refused::Full), Page::Busy);
    assert_eq!(refusal_page(Refused::NoClient), Page::Unavailable);
    assert_eq!(refusal_page(Refused::Gone), Page::NotFound);
}
