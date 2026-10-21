pub const MAX_HEAD: usize = 8 * 1024;

pub const SECURITY_HEADERS: &str = "Content-Security-Policy: default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Resource-Policy: same-origin\r\nX-Frame-Options: DENY\r\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reject {
    pub code: u16,
    pub reason: &'static str,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Head {
    pub method: String,
    pub path: String,
    pub host: Option<String>,
    pub origin: Option<String>,
    pub fetch_site: Option<String>,
    pub upgrade: bool,
    pub gzip: bool,
}

pub fn parse(bytes: &[u8]) -> Result<Head, Reject> {
    let mut headers = [httparse::EMPTY_HEADER; 48];
    let mut req = httparse::Request::new(&mut headers);
    match req.parse(bytes) {
        Ok(httparse::Status::Complete(_)) => {}
        _ => {
            return Err(Reject {
                code: 400,
                reason: "bad request",
            });
        }
    }
    let mut head = Head {
        method: req.method.unwrap_or_default().to_string(),
        path: req.path.unwrap_or_default().to_string(),
        ..Head::default()
    };
    for h in req.headers.iter() {
        let value = String::from_utf8_lossy(h.value).trim().to_string();
        let name = h.name.to_ascii_lowercase();
        let once = |slot: &mut Option<String>| {
            if slot.is_some() {
                return Err(Reject {
                    code: 400,
                    reason: "repeated header",
                });
            }
            *slot = Some(value.clone());
            Ok(())
        };
        match name.as_str() {
            "host" => once(&mut head.host)?,
            "origin" => once(&mut head.origin)?,
            "sec-fetch-site" => once(&mut head.fetch_site)?,
            "upgrade" => {
                head.upgrade |= value.eq_ignore_ascii_case(crate::transport::ws::UPGRADE_TOKEN)
            }
            "accept-encoding" => {
                head.gzip |= value.split(',').any(|v| v.trim().starts_with("gzip"))
            }
            _ => {}
        }
    }
    Ok(head)
}

pub fn allowed_hosts(port: u16) -> [String; 3] {
    [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ]
}

pub fn run_prefix() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("operating system random source is available");
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("/r-{hex}")
}

pub fn is_file(path: &str) -> bool {
    path.strip_prefix('/')
        .is_some_and(|name| !name.contains('/') && name.contains('.'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Page(String),
    Asset(String),
    Api,
    Redirect(String),
    NotFound,
    Reject,
}

fn climbs(path: &str) -> bool {
    percent_encoding::percent_decode_str(path)
        .decode_utf8_lossy()
        .split(['/', '\\'])
        .any(|segment| segment == "..")
}

fn without_run(path: &str) -> &str {
    match path.strip_prefix("/r-") {
        Some(rest) => rest.find('/').map_or("/", |i| &rest[i..]),
        None => path,
    }
}

pub fn route(target: &str, prefix: &str, upgrade: bool) -> Decision {
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let inner = path
        .strip_prefix(prefix)
        .filter(|rest| rest.starts_with('/'));
    if inner == Some("/api") {
        return Decision::Api;
    }
    if !path.starts_with('/') || climbs(path) {
        return Decision::Reject;
    }
    if let Some(inner) = inner {
        return if is_file(inner) {
            Decision::Asset(inner.to_string())
        } else {
            Decision::Page(inner.to_string())
        };
    }
    let rest = without_run(path);
    let last = rest.rsplit('/').next().unwrap_or("");
    if rest == "/api" || (upgrade && last == "api") || is_file(rest) {
        return Decision::NotFound;
    }
    let query = target
        .split_once('?')
        .map(|(_, q)| format!("?{}", q.split('#').next().unwrap_or("")))
        .unwrap_or_default();
    Decision::Redirect(format!("{prefix}{rest}{query}"))
}

pub fn check(head: &Head, port: u16, prefix: &str) -> Result<Decision, Reject> {
    let forbidden = |reason| Reject { code: 403, reason };
    let host = head.host.as_deref().ok_or(Reject {
        code: 400,
        reason: "no Host header",
    })?;
    if !allowed_hosts(port)
        .iter()
        .any(|h| h.eq_ignore_ascii_case(host))
    {
        return Err(forbidden("this page is served for 127.0.0.1 only"));
    }
    if let Some(site) = head.fetch_site.as_deref()
        && !matches!(site, "same-origin" | "none")
    {
        return Err(forbidden("requests from other sites are refused"));
    }
    if head.method != "GET" {
        return Err(Reject {
            code: 405,
            reason: "only GET is supported",
        });
    }
    match route(&head.path, prefix, head.upgrade) {
        Decision::Api => {
            if !head.upgrade {
                return Err(Reject {
                    code: 426,
                    reason: "a WebSocket upgrade is needed",
                });
            }
            let expected = format!("http://{host}");
            if head.origin.as_deref() != Some(expected.as_str()) {
                return Err(forbidden("the WebSocket must come from this page"));
            }
            Ok(Decision::Api)
        }
        Decision::Reject => Err(Reject {
            code: 400,
            reason: "bad path",
        }),
        Decision::NotFound => Err(Reject {
            code: 404,
            reason: "not found",
        }),
        other => Ok(other),
    }
}

pub fn reason_phrase(code: u16) -> &'static str {
    match code {
        200 => "OK",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        426 => "Upgrade Required",
        _ => "Error",
    }
}

pub fn response(code: u16, mime: &str, extra: &str, body_len: usize) -> String {
    format!(
        "HTTP/1.1 {code} {}\r\nContent-Type: {mime}\r\nContent-Length: {body_len}\r\n{SECURITY_HEADERS}{extra}Connection: close\r\n\r\n",
        reason_phrase(code)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(lines: &[&str]) -> Head {
        let text = format!("{}\r\n\r\n", lines.join("\r\n"));
        parse(text.as_bytes()).unwrap()
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
        let twice = "GET / HTTP/1.1\r\nHost: 127.0.0.1:4000\r\nHost: evil\r\n\r\n";
        assert_eq!(parse(twice.as_bytes()).unwrap_err().code, 400);
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
    fn every_response_carries_the_security_headers() {
        let r = response(200, "text/html", "", 10);
        for h in [
            "Content-Security-Policy: default-src 'none'; script-src 'self' 'wasm-unsafe-eval'",
            "X-Content-Type-Options: nosniff",
            "Referrer-Policy: no-referrer",
            "Cross-Origin-Opener-Policy: same-origin",
            "Cross-Origin-Resource-Policy: same-origin",
            "frame-ancestors 'none'",
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
            "..", ".", "%2e", "%2E", "%2f", "%5c", "%", "/", "\\", "api", "a", "x.js", "?", "#",
            "r-", run, "%252e", "", "%00", "\u{e9}",
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
}
