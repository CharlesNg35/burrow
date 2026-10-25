mod common;

use std::sync::Arc;

use burrow_web::{Redirect, Site, SiteOptions};
use common::*;

fn with(site: Site, options: SiteOptions) -> Site {
    Site { options, ..site }
}

fn paths(list: &[&str], app_path: Option<&str>) -> SiteOptions {
    SiteOptions {
        paths: list.iter().map(|p| p.to_string()).collect(),
        app_path: app_path.map(str::to_string),
        ..SiteOptions::default()
    }
}

struct Two {
    proxy: burrow_web::Proxy<Fake>,
    up: Arc<Fake>,
}

async fn two(sites: impl FnOnce() -> Vec<Site>) -> Two {
    let blog = http_backend().await;
    let admin = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", blog.addr);
    up.serve("admin", admin.addr);
    up.serve("api", admin.addr);
    let proxy = start(|s| s.sites = sites(), up.clone()).await;
    Two { proxy, up }
}

async fn get(p: &burrow_web::Proxy<Fake>, target: &str, extra: &str) -> String {
    over_tls(
        p.https_addr().unwrap(),
        "example.com",
        &format!("GET {target} HTTP/1.1\r\nHost: example.com\r\n{extra}Connection: close\r\n\r\n"),
    )
    .await
}

fn served_by(t: &Two) -> Vec<String> {
    t.up.visitors
        .lock()
        .unwrap()
        .iter()
        .map(|(s, _)| s.clone())
        .collect()
}

#[tokio::test]
async fn a_merged_double_slash_reaches_the_admin_service_not_the_open_one() {
    let t = two(|| {
        vec![
            https_site("blog", &["example.com"]),
            with(
                https_site("admin", &["example.com"]),
                paths(&["/admin"], None),
            ),
        ]
    })
    .await;
    let r = get(&t.proxy, "//admin", "").await;
    assert!(r.contains("\nGET /admin\n"), "{r}");
    let r = get(&t.proxy, "//admin//x", "").await;
    assert!(r.contains("\nGET /admin/x\n"), "{r}");
    let r = get(&t.proxy, "/adminx", "").await;
    assert!(r.contains("\nGET /adminx\n"), "{r}");
    assert_eq!(served_by(&t), ["admin", "admin", "blog"]);
    for path in ["/%2fx", "/%2Fx"] {
        let r = get(&t.proxy, path, "").await;
        assert!(r.contains("\nGET /%2Fx\n"), "{r}");
    }
}

#[tokio::test]
async fn app_path_replaces_the_prefix_and_names_it_in_x_forwarded_prefix() {
    let t = two(|| {
        vec![
            https_site("blog", &["example.com"]),
            with(
                https_site("api", &["example.com"]),
                paths(&["/api", "/v2"], Some("/")),
            ),
        ]
    })
    .await;
    let r = get(
        &t.proxy,
        "/api/users?page=2",
        "X-Forwarded-Prefix: /evil\r\n",
    )
    .await;
    assert!(r.contains("\nGET /users?page=2\n"), "{r}");
    assert!(r.contains("x-forwarded-prefix: /api\n"), "{r}");
    assert!(!r.contains("/evil"), "{r}");
    let r = get(&t.proxy, "/v2", "").await;
    assert!(r.contains("\nGET /\n"), "{r}");
    assert!(r.contains("x-forwarded-prefix: /v2\n"), "{r}");
    let r = get(&t.proxy, "/", "X-Forwarded-Prefix: /evil\r\n").await;
    assert!(!r.contains("x-forwarded-prefix"), "{r}");
}

#[tokio::test]
async fn redirects_keep_the_rest_and_are_matched_before_app_path() {
    let mut options = paths(&["/api"], Some("/v1"));
    options.redirects = vec![
        Redirect {
            from: "/api/old".into(),
            to: "/api/new".into(),
            permanent: false,
        },
        Redirect {
            from: "/docs".into(),
            to: "https://docs.example.net".into(),
            permanent: true,
        },
    ];
    options.redirect_from = vec!["www.example.com".into()];
    let t = two(move || vec![with(https_site("api", &["example.com"]), options)]).await;
    let r = get(&t.proxy, "/api/old/page?q=1", "").await;
    assert!(r.starts_with("HTTP/1.1 307"), "{r}");
    assert!(r.contains("location: /api/new/page?q=1\r\n"), "{r}");
    let r = get(&t.proxy, "/docs//guide", "").await;
    assert!(r.starts_with("HTTP/1.1 308"), "{r}");
    assert!(
        r.contains("location: https://docs.example.net/guide\r\n"),
        "{r}"
    );
    let r = get(&t.proxy, "/api/users", "").await;
    assert!(r.contains("\nGET /v1/users\n"), "{r}");
    let r = over_tls(
        t.proxy.https_addr().unwrap(),
        "www.example.com",
        "GET /a?b HTTP/1.1\r\nHost: www.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 308"), "{r}");
    assert!(r.contains("location: https://example.com/a?b\r\n"), "{r}");
    let r = get(&t.proxy, "/caf\u{e9}-x", "").await;
    assert!(r.starts_with("HTTP/1.1 404"), "{r}");
}

fn limited(max: u64) -> SiteOptions {
    SiteOptions {
        max_body_size: Some(max),
        ..SiteOptions::default()
    }
}

#[tokio::test]
async fn a_body_over_max_body_size_is_413_without_100_continue() {
    let t = two(|| vec![with(https_site("blog", &["example.com"]), limited(10))]).await;
    let addr = t.proxy.https_addr().unwrap();
    let r = over_tls(
        addr,
        "example.com",
        "POST /up HTTP/1.1\r\nHost: example.com\r\nContent-Length: 11\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 413"), "{r}");
    assert!(!r.contains("100 Continue"), "{r}");
    let r = over_tls(
        addr,
        "example.com",
        "POST /up HTTP/1.1\r\nHost: example.com\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n6\r\n012345\r\n6\r\n678901\r\n0\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 413"), "{r}");
    let r = over_tls(
        addr,
        "example.com",
        "POST /up HTTP/1.1\r\nHost: example.com\r\nContent-Length: 10\r\nConnection: close\r\n\r\n0123456789",
    )
    .await;
    assert!(r.contains("body-len: 10\n"), "{r}");
}

#[tokio::test]
async fn maintenance_answers_503_without_asking_the_app() {
    let options = SiteOptions {
        maintenance: true,
        ..SiteOptions::default()
    };
    let t = two(|| vec![with(https_site("blog", &["example.com"]), options)]).await;
    let r = get(&t.proxy, "/", "").await;
    assert!(r.starts_with("HTTP/1.1 503"), "{r}");
    assert!(r.contains("retry-after: 60\r\n"), "{r}");
    assert!(served_by(&t).is_empty());
    let answered = t.up.answered.lock().unwrap().clone();
    assert_eq!(answered, [("blog".to_string(), 503)]);
}

fn rule(name: &'static str, value: &'static str) -> burrow_web::HeaderRule {
    burrow_web::HeaderRule {
        name: http::HeaderName::from_static(name),
        value: (!value.is_empty()).then(|| http::HeaderValue::from_static(value)),
    }
}

#[tokio::test]
async fn header_rules_reach_the_app_and_the_visitor_and_hsts_only_over_https() {
    let options = SiteOptions {
        hsts: true,
        request_headers: vec![rule("host", "internal.lan"), rule("x-drop", "")],
        response_headers: vec![rule("x-frame-options", "DENY"), rule("x-from-app", "")],
        ..SiteOptions::default()
    };
    let app = http_backend().await;
    let (up, _) = Fake::new(4);
    up.serve("blog", app.addr);
    let site = Site {
        https_redirect: false,
        ..with(https_site("blog", &["example.com"]), options)
    };
    let proxy = start_with(|s| s.sites = vec![site], up, fixed(&["example.com"]), true).await;
    let r = get(&proxy, "/", "X-Drop: 1\r\n").await;
    assert!(r.contains("host: internal.lan\n"), "{r}");
    assert!(r.contains("x-forwarded-host: example.com\n"), "{r}");
    assert!(!r.contains("x-drop"), "{r}");
    assert!(r.contains("x-frame-options: DENY\r\n"), "{r}");
    assert!(!r.contains("x-from-app"), "{r}");
    assert!(
        r.contains("strict-transport-security: max-age=31536000\r\n"),
        "{r}"
    );
    let plain = over_plain(
        proxy.http_addr().unwrap(),
        "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(plain.starts_with("HTTP/1.1 200"), "{plain}");
    assert!(!plain.contains("strict-transport-security"), "{plain}");
}

#[tokio::test]
async fn a_slow_app_gets_504_after_its_response_header_timeout() {
    let options = SiteOptions {
        response_header_timeout: std::time::Duration::from_millis(200),
        ..SiteOptions::default()
    };
    let t = two(|| vec![with(https_site("blog", &["example.com"]), options)]).await;
    let r = get(&t.proxy, "/slow/2000", "").await;
    assert!(r.starts_with("HTTP/1.1 504"), "{r}");
    let r = get(&t.proxy, "/slow/10", "").await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
}

#[tokio::test]
async fn an_h2c_upgrade_is_never_passed_to_the_app_and_the_request_is_served_plainly() {
    let t = two(|| vec![https_site("blog", &["example.com"])]).await;
    let r = get(
        &t.proxy,
        "/",
        "Upgrade: h2c\r\nHTTP2-Settings: AAMAAABkAARAAAAAAAIAAAAA\r\nConnection: Upgrade, HTTP2-Settings\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let app_saw = r.split("\r\n\r\n").nth(1).unwrap_or_default();
    assert!(app_saw.starts_with("GET /\n"), "{r}");
    assert!(!app_saw.contains("upgrade"), "{r}");
    assert!(!app_saw.contains("http2-settings"), "{r}");
}

#[tokio::test]
async fn encoded_slashes_and_segment_parameters_cannot_slip_past_a_path_route() {
    let t = two(|| {
        vec![
            https_site("blog", &["example.com"]),
            with(
                https_site("admin", &["example.com"]),
                paths(&["/admin", "/wp-admin"], None),
            ),
        ]
    })
    .await;
    for (sent, seen) in [
        ("/admin%2F", "GET /admin%2F\n"),
        ("/admin%2fsecret", "GET /admin%2Fsecret\n"),
        ("/wp-admin%2Finstall.php", "GET /wp-admin%2Finstall.php\n"),
        ("/admin;x=1", "GET /admin;x=1\n"),
        ("/admin%5Cx", "GET /admin%5Cx\n"),
    ] {
        let r = get(&t.proxy, sent, "").await;
        assert!(r.contains(seen), "{sent}: {r}");
    }
    assert_eq!(served_by(&t), ["admin"; 5]);
    let r = get(&t.proxy, "/..;/admin", "").await;
    assert!(r.starts_with("HTTP/1.1 400"), "{r}");
}

#[tokio::test]
async fn a_path_route_matches_any_letter_case_and_the_app_keeps_the_case_sent() {
    let t = two(|| {
        vec![
            https_site("blog", &["example.com"]),
            with(
                https_site("admin", &["example.com"]),
                paths(&["/wp-admin"], Some("/")),
            ),
        ]
    })
    .await;
    let r = get(&t.proxy, "/WP-ADMIN/Install.php", "").await;
    assert!(r.contains("\nGET /Install.php\n"), "{r}");
    assert!(r.contains("x-forwarded-prefix: /wp-admin\n"), "{r}");
    assert_eq!(served_by(&t), ["admin"]);
}

#[tokio::test]
async fn an_outpost_path_on_a_wildcard_site_reaches_its_own_service() {
    let t = two(|| {
        vec![
            https_site("blog", &["*.apps.example.com"]),
            with(
                https_site("admin", &["*.apps.example.com"]),
                paths(&["/outpost.goauthentik.io"], None),
            ),
        ]
    })
    .await;
    let r = over_tls(
        t.proxy.https_addr().unwrap(),
        "grafana.apps.example.com",
        "GET /outpost.goauthentik.io/start?rd=x HTTP/1.1\r\nHost: grafana.apps.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        r.contains("\nGET /outpost.goauthentik.io/start?rd=x\n"),
        "{r}"
    );
    let r = over_tls(
        t.proxy.https_addr().unwrap(),
        "grafana.apps.example.com",
        "GET / HTTP/1.1\r\nHost: grafana.apps.example.com\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert_eq!(served_by(&t), ["admin", "blog"]);
}
