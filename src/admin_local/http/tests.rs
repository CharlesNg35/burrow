use super::*;

fn request(lines: &[&str]) -> ::http::Request<()> {
    let mut parts = lines[0].split(' ');
    let mut b = ::http::Request::builder()
        .method(parts.next().unwrap())
        .uri(parts.next().unwrap());
    for l in &lines[1..] {
        let (k, v) = l.split_once(": ").unwrap();
        b = b.header(k, v);
    }
    b.body(()).unwrap()
}

fn head(lines: &[&str]) -> Head {
    head_of(&request(lines)).unwrap()
}

#[test]
fn only_the_local_host_name_is_served() {
    for host in [
        "127.0.0.1:4000",
        "localhost:4000",
        "[::1]:4000",
        "LOCALHOST:4000",
    ] {
        let h = head(&["GET / HTTP/1.1", &format!("Host: {host}")]);
        assert_eq!(
            check(&h, 4000, ""),
            Ok(Decision::Page("/".into())),
            "{host}"
        );
    }
    for host in [
        "evil.example",
        "127.0.0.1:4001",
        "127.0.0.1",
        "evil.example:4000",
        "10.0.0.1:4000",
    ] {
        let h = head(&["GET / HTTP/1.1", &format!("Host: {host}")]);
        assert_eq!(check(&h, 4000, "").unwrap_err().code, 403, "{host}");
    }
    let none = head(&["GET / HTTP/1.1"]);
    assert_eq!(check(&none, 4000, "").unwrap_err().code, 400);
    let twice = request(&["GET / HTTP/1.1", "Host: 127.0.0.1:4000", "Host: evil"]);
    assert_eq!(head_of(&twice).unwrap_err().code, 400);
}

#[test]
fn the_websocket_needs_this_origin_and_no_cross_site_fetch() {
    let ws = |origin: Option<&str>, site: Option<&str>| {
        let mut l = vec![
            "GET /api HTTP/1.1".to_string(),
            "Host: 127.0.0.1:4000".into(),
            "Upgrade: websocket".into(),
            "Connection: Upgrade".into(),
        ];
        if let Some(o) = origin {
            l.push(format!("Origin: {o}"));
        }
        if let Some(s) = site {
            l.push(format!("Sec-Fetch-Site: {s}"));
        }
        let refs: Vec<&str> = l.iter().map(String::as_str).collect();
        check(&head(&refs), 4000, "")
    };
    assert_eq!(
        ws(Some("http://127.0.0.1:4000"), Some("same-origin")),
        Ok(Decision::Api)
    );
    assert_eq!(ws(Some("http://127.0.0.1:4000"), None), Ok(Decision::Api));
    for (o, s) in [
        (Some("http://evil.example"), None),
        (Some("http://localhost:4000"), None),
        (Some("https://127.0.0.1:4000"), None),
        (None, None),
        (Some("null"), None),
        (Some("http://127.0.0.1:4000"), Some("cross-site")),
        (Some("http://127.0.0.1:4000"), Some("same-site")),
    ] {
        assert_eq!(ws(o, s).unwrap_err().code, 403, "{o:?} {s:?}");
    }
    let plain = head(&["GET /api HTTP/1.1", "Host: 127.0.0.1:4000"]);
    assert_eq!(check(&plain, 4000, "").unwrap_err().code, 426);
    let post = head(&["POST / HTTP/1.1", "Host: 127.0.0.1:4000"]);
    assert_eq!(check(&post, 4000, "").unwrap_err().code, 405);
    let up = head(&["GET /../etc/passwd HTTP/1.1", "Host: 127.0.0.1:4000"]);
    assert_eq!(check(&up, 4000, "").unwrap_err().code, 400);
}

#[test]
fn the_launch_page_can_open_the_ui_while_other_cross_site_requests_are_refused() {
    let get = |site: &str, mode: &str, dest: &str| {
        check(
            &head(&[
                "GET /r-abc/ HTTP/1.1",
                "Host: 127.0.0.1:4000",
                &format!("Sec-Fetch-Site: {site}"),
                &format!("Sec-Fetch-Mode: {mode}"),
                &format!("Sec-Fetch-Dest: {dest}"),
            ]),
            4000,
            "/r-abc",
        )
    };
    assert_eq!(
        get("cross-site", "navigate", "document"),
        Ok(Decision::Page("/".into()))
    );
    for (site, mode, dest) in [
        ("cross-site", "no-cors", "script"),
        ("cross-site", "cors", "empty"),
        ("cross-site", "navigate", "iframe"),
        ("same-site", "no-cors", "image"),
    ] {
        assert_eq!(
            get(site, mode, dest).unwrap_err().code,
            403,
            "{site} {mode} {dest}"
        );
    }
    let ws = head(&[
        "GET /r-abc/api HTTP/1.1",
        "Host: 127.0.0.1:4000",
        "Upgrade: websocket",
        "Connection: Upgrade",
        "Origin: http://127.0.0.1:4000",
        "Sec-Fetch-Site: cross-site",
        "Sec-Fetch-Mode: navigate",
        "Sec-Fetch-Dest: document",
    ]);
    assert_eq!(check(&ws, 4000, "/r-abc").unwrap_err().code, 403);
}

#[test]
fn every_response_carries_the_security_headers() {
    let r = response(200, "text/html", &[], vec![0u8; 10]);
    let r: String = r
        .headers()
        .iter()
        .map(|(k, v)| format!("{k}: {}\r\n", v.to_str().unwrap()))
        .collect();
    for h in [
        "content-security-policy: default-src 'none'; script-src 'self' 'wasm-unsafe-eval'",
        "x-content-type-options: nosniff",
        "referrer-policy: no-referrer",
        "cross-origin-opener-policy: same-origin",
        "cross-origin-resource-policy: same-origin",
        "frame-ancestors 'none'",
        "content-length: 10",
        "connection: close",
    ] {
        assert!(r.contains(h), "{h}");
    }
    assert!(!r.to_ascii_lowercase().contains("set-cookie"));
    assert!(!r.to_ascii_lowercase().contains("access-control"));
}

#[test]
fn only_this_run_s_prefix_serves_pages_and_the_api() {
    let run = "/r-0123456789abcdef0123456789abcdef";
    let get = |path: &str, upgrade: bool| {
        let mut lines = vec![
            format!("GET {path} HTTP/1.1"),
            "Host: 127.0.0.1:4000".to_string(),
        ];
        if upgrade {
            lines.push("Upgrade: websocket".into());
            lines.push("Origin: http://127.0.0.1:4000".into());
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        check(&head(&refs), 4000, run)
    };
    assert_eq!(
        get(&format!("{run}/"), false),
        Ok(Decision::Page("/".into()))
    );
    assert_eq!(
        get(&format!("{run}/services/web?tab=x"), false),
        Ok(Decision::Page("/services/web".into()))
    );
    assert_eq!(get(&format!("{run}/api"), true), Ok(Decision::Api));
    for elsewhere in ["/api", "/r-old/api", "/services/api"] {
        assert_eq!(get(elsewhere, true).unwrap_err().code, 404, "{elsewhere}");
    }
    assert_eq!(get("/", false), Ok(Decision::Redirect(format!("{run}/"))));
    assert_eq!(
        get("/services/web?tab=secrets", false),
        Ok(Decision::Redirect(format!(
            "{run}/services/web?tab=secrets"
        )))
    );
    assert_eq!(
        get("/r-feedfacefeedfacefeedfacefeedface/clients", false),
        Ok(Decision::Redirect(format!("{run}/clients")))
    );
    assert_eq!(get("/app-1234.js", false).unwrap_err().code, 404);
    assert_eq!(
        get(&format!("{run}/services/api.v2"), false),
        Ok(Decision::Page("/services/api.v2".into()))
    );
    assert_eq!(
        get("/services/api.v2?tab=x", false),
        Ok(Decision::Redirect(format!("{run}/services/api.v2?tab=x")))
    );
    assert_eq!(
        get("/clients/home.lan", false),
        Ok(Decision::Redirect(format!("{run}/clients/home.lan")))
    );
    for (page, moved) in [
        ("/services/api", "/services/api"),
        (
            "/r-feedfacefeedfacefeedfacefeedface/clients/api",
            "/clients/api",
        ),
        ("/services/a..b", "/services/a..b"),
    ] {
        assert_eq!(
            get(page, false),
            Ok(Decision::Redirect(format!("{run}{moved}"))),
            "{page}"
        );
    }
    assert_eq!(
        get(&format!("{run}/services/a..b"), false),
        Ok(Decision::Page("/services/a..b".into()))
    );
    for bad in [
        "/../x".to_string(),
        format!("{run}/../x"),
        "/a/%2e%2e/x".to_string(),
        format!("{run}/.%2E/x"),
        "/a%2f..%2fb".to_string(),
    ] {
        assert_eq!(get(&bad, false).unwrap_err().code, 400, "{bad}");
    }
    for page in ["/", "/services/api.v2", "/clients/home.lan", "/services"] {
        assert!(!is_file(page), "{page}");
    }
    for file in ["/app-1234.js", "/index.html", "/fonts-license.txt"] {
        assert!(is_file(file), "{file}");
    }
    assert_eq!(
        get(&format!("{run}x/"), false),
        Ok(Decision::Redirect(format!("{run}/")))
    );
}

#[test]
fn every_past_path_bug_is_decided_by_route() {
    let run = "/r-0123456789abcdef0123456789abcdef";
    let page = |p: &str| Decision::Page(p.to_string());
    let asset = |p: &str| Decision::Asset(p.to_string());
    let moved = |p: &str| Decision::Redirect(format!("{run}{p}"));
    for (target, upgrade, prefixed, plain) in [
        ("/", false, page("/"), moved("/")),
        (
            "/services/web?tab=x",
            false,
            page("/services/web"),
            moved("/services/web?tab=x"),
        ),
        ("/app-1.js", false, asset("/app-1.js"), Decision::NotFound),
        (
            "/index.html",
            false,
            asset("/index.html"),
            Decision::NotFound,
        ),
        ("/x.y.z", false, asset("/x.y.z"), Decision::NotFound),
        (
            "/services/x.y.z",
            false,
            page("/services/x.y.z"),
            moved("/services/x.y.z"),
        ),
        ("/api", true, Decision::Api, Decision::NotFound),
        ("/api", false, Decision::Api, Decision::NotFound),
        ("/api/", true, page("/api/"), moved("/api/")),
        (
            "/services/api",
            true,
            page("/services/api"),
            Decision::NotFound,
        ),
        (
            "/services/api",
            false,
            page("/services/api"),
            moved("/services/api"),
        ),
        ("/..", false, Decision::Reject, Decision::Reject),
        ("/a/../b", false, Decision::Reject, Decision::Reject),
        ("/a/%2e%2e/b", false, Decision::Reject, Decision::Reject),
        ("/a/%2E./b", false, Decision::Reject, Decision::Reject),
        ("/a/.%2E/b", false, Decision::Reject, Decision::Reject),
        ("/a%2f..%2fb", false, Decision::Reject, Decision::Reject),
        ("/a%5c..%5cb", false, Decision::Reject, Decision::Reject),
        ("/a\\..\\b", false, Decision::Reject, Decision::Reject),
        (
            "/services/a..b",
            false,
            page("/services/a..b"),
            moved("/services/a..b"),
        ),
        ("//services", false, page("//services"), moved("//services")),
    ] {
        assert_eq!(
            route(&format!("{run}{target}"), run, upgrade),
            prefixed,
            "{run}{target}"
        );
        assert_eq!(route(target, run, upgrade), plain, "{target}");
    }
    let stale = "/r-feedfacefeedfacefeedfacefeedface";
    assert_eq!(
        route(&format!("{stale}/clients"), run, false),
        moved("/clients")
    );
    assert_eq!(
        route(&format!("{stale}/api"), run, true),
        Decision::NotFound
    );
    assert_eq!(route(&format!("{run}x/"), run, false), moved("/"));
    assert_eq!(route("services", run, false), Decision::Reject);
}

#[test]
fn random_paths_never_escape_and_reach_the_api_only_exactly() {
    let run = "/r-0123456789abcdef0123456789abcdef";
    let alphabet = [
        "..", ".", "%2e", "%2E", "%2f", "%5c", "%", "/", "\\", "api", "a", "x.js", "?", "#", "r-",
        run, "%252e", "", "%00", "\u{e9}",
    ];
    arbtest::arbtest(|u| {
        let n = u.int_in_range(0..=8)?;
        let mut target = String::from(if u.arbitrary()? { run } else { "" });
        for _ in 0..n {
            target.push('/');
            let parts = u.int_in_range(1..=3)?;
            for _ in 0..parts {
                target.push_str(u.choose(&alphabet)?);
            }
        }
        let upgrade = u.arbitrary()?;
        let path = target.split(['?', '#']).next().unwrap_or("");
        match route(&target, run, upgrade) {
            Decision::Api => assert_eq!(path, format!("{run}/api"), "{target}"),
            Decision::Page(p) | Decision::Asset(p) => {
                assert!(!climbs(&p), "{target} escapes as {p}");
                assert!(p.starts_with('/'), "{target}");
            }
            Decision::Redirect(to) => {
                assert!(to.starts_with(&format!("{run}/")), "{target} -> {to}");
                let to_path = to.split(['?', '#']).next().unwrap_or("");
                assert!(!climbs(to_path), "{target} -> {to}");
                assert_ne!(to_path, format!("{run}/api"), "{target}");
            }
            Decision::NotFound | Decision::Reject => {}
        }
        Ok(())
    })
    .budget_ms(500);
}
