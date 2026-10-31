use super::tests::{errors, load_in};
use super::tests_site_keys::{errs, run, warnings};

fn svc(name: &str, web: &str) -> String {
    format!("  {name}:\n    client: home\n    target: 8080\n    web: {web}\n")
}

#[test]
fn address_ranges_must_parse_and_the_whole_internet_is_a_warning() {
    assert_eq!(
        errs(
            "  trusted_proxies: [cloudflare]\n",
            &svc("blog", "example.com")
        )
        .len(),
        1
    );
    let (_d, o) = run(
        "  trusted_proxies: [0.0.0.0/0]\n  proxy_protocol_from: [0.0.0.0/0]\n",
        &svc("blog", "{ hosts: [example.com], challenge: http }"),
    );
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
    let w = warnings(&o);
    assert!(w.contains(&"web.proxy_protocol_from holds 0.0.0.0/0: every visitor would have to send a PROXY header".to_string()), "{w:#?}");
    assert!(w.contains(&"web.trusted_proxies holds 0.0.0.0/0: every visitor could set its own address with X-Forwarded-For".to_string()), "{w:#?}");
}

#[test]
fn a_plain_site_redirects_to_https_only_behind_a_trusted_proxy() {
    let plain = svc(
        "blog",
        "{ hosts: [blog.example.com], tls: off, https_redirect: true }",
    );
    assert_eq!(
        errs("", &plain),
        [
            "services.blog.web.https_redirect: true on a plain-HTTP site needs web.trusted_proxies: burrow can only tell which visitors used HTTPS at a proxy it trusts; add trusted_proxies, or set https_redirect: false"
        ]
    );
    let (_d, o) = run("  trusted_proxies: [173.245.48.0/20]\n", &plain);
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
    assert!(warnings(&o).contains(&"services.blog: http:// visitors are sent to https:// by the X-Forwarded-Proto of the trusted proxies; if they do not send it, every request redirects".to_string()), "{:#?}", warnings(&o));
}

#[test]
fn trusted_proxies_bring_one_note_listing_the_sites_on_the_automatic_tls_check() {
    let s = svc("kiosk", "kiosk.example.com")
        + &svc("shop", "shop.example.com")
        + &svc("news", "{ hosts: [news.example.com], challenge: http }")
        + &svc("nas", "{ hosts: [nas.example.com], tls: passthrough }")
        + &svc("lan", "{ hosts: [lan.example.com], tls: off }");
    let note = |o: &crate::config::Outcome| {
        warnings(o)
            .into_iter()
            .filter(|w| w.starts_with("a proxy is in front"))
            .collect::<Vec<_>>()
    };
    let (_d, o) = run("  trusted_proxies: [173.245.48.0/20]\n", &s);
    assert_eq!(
        note(&o),
        [
            "a proxy is in front (web.trusted_proxies is set); if it does the HTTPS, like Cloudflare's proxy, Let's Encrypt's TLS check cannot reach burrow: set challenge: http (with http: 80) or challenge: dns on these sites: kiosk, shop"
        ]
    );
    let (_d, o) = run("", &s);
    assert!(note(&o).is_empty());
    let (_d, o) = run(
        "  trusted_proxies: [173.245.48.0/20]\n",
        &svc("news", "{ hosts: [news.example.com], challenge: tls }"),
    );
    assert!(note(&o).is_empty(), "any challenge silences it");
}

#[test]
fn a_redirect_name_is_its_own_route_unless_a_service_serves_the_same_prefix() {
    let s = svc("site", "example.com") + &svc("api", "{ hosts: [example.com], paths: [/api] }");
    let ok = "  redirect_names:\n    example.com/shop: \"https://shop.example.net/\"\n    old.example.com: \"https://example.com\"\n";
    assert_eq!(errs(ok, &s), Vec::<String>::new());
    assert_eq!(
        errs(
            "  redirect_names:\n    example.com/api: \"https://api.example.net\"\n",
            &s
        ),
        ["web.redirect_names: \"example.com/api\" is also served by service \"api\""]
    );
    assert_eq!(
        errs(
            "  redirect_names:\n    example.com: \"https://example.net\"\n",
            &s
        ),
        ["web.redirect_names: \"example.com\" is also served by service \"site\""]
    );
    assert_eq!(
        errs(
            "  redirect_names:\n    old.example.com: new.example.com\n",
            &s
        ),
        ["redirect target \"new.example.com\" must be a full URL (https://...) or a path (/...)"]
    );
}

#[test]
fn names_that_only_redirect_need_a_certificate_like_any_https_name() {
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "listen: 7835\nweb:\n  https: 443\n  redirect_names:\n    old.example.com: \"https://example.com\"\nservices:\n{}",
        svc(
            "blog",
            "{ hosts: [example.com], tls: { cert: b.pem, key: b.key }, redirect_from: [www.example.com] }"
        )
    );
    let o = load_in(dir.path(), &text);
    let found: Vec<String> = errors(&o)
        .into_iter()
        .filter(|e| e.contains("has no certificate"))
        .collect();
    assert_eq!(
        found,
        [
            "host \"old.example.com\" (web.redirect_names) has no certificate; add web.letsencrypt for automatic certificates, or an entry in web.certificates whose hosts lists it",
            "host \"www.example.com\" (service \"blog\") has no certificate; add web.letsencrypt for automatic certificates, or an entry in web.certificates whose hosts lists it",
        ]
    );
}

#[test]
fn a_site_path_inside_the_tunnel_path_is_refused() {
    let (_d, o) = run(
        "",
        &(svc("app", "{ hosts: [example.com], paths: [/burrow/x] }") + &svc("site", "example.com")),
    );
    assert!(errors(&o).is_empty(), "no tunnel yet: {:#?}", errors(&o));
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "websocket: {{ listen: 443, host: example.com, path: /burrow }}\n{}  letsencrypt: {{ email: ops@example.com }}\nservices:\n{}{}",
        "web:\n  https: 443\n  http: 80\n",
        svc("app", "{ hosts: [example.com], paths: [/burrow/x] }"),
        svc("site", "example.com")
    );
    let o = load_in(dir.path(), &text);
    assert!(
        errors(&o).contains(&"websocket.path \"/burrow\" on \"example.com\" is also served by service \"app\"; give the tunnel another path".to_string()),
        "{:#?}",
        errors(&o)
    );
}

#[test]
fn an_access_log_in_a_missing_folder_is_a_warning() {
    let (_d, o) = run(
        "  access_log: logs/access.log\n",
        &svc("blog", "example.com"),
    );
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
    assert!(
        warnings(&o).contains(
            &"web.access_log: the folder of logs/access.log does not exist yet".to_string()
        ),
        "{:#?}",
        warnings(&o)
    );
}

#[test]
fn a_redirect_name_differing_only_by_letter_case_is_the_same_route() {
    let s = svc("site", "example.com") + &svc("api", "{ hosts: [example.com], paths: [/api] }");
    assert_eq!(
        errs(
            "  redirect_names:\n    example.com/API: \"https://api.example.net\"\n",
            &s
        ),
        ["web.redirect_names: \"example.com/API\" is also served by service \"api\""]
    );
    assert_eq!(
        errs(
            "  redirect_names:\n    example.com/Shop: \"https://a.example.net\"\n    Example.com/shop/: \"https://b.example.net\"\n",
            &s
        ),
        [
            "web.redirect_names: \"Example.com/shop/\" and \"example.com/Shop\" are the same route; paths match whatever the letter case"
        ]
    );
}
