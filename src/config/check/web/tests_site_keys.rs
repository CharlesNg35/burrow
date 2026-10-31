use super::tests::{errors, load_in};
use crate::config::Outcome;

pub(super) const TOP: &str =
    "listen: 7835\nweb:\n  https: 443\n  http: 80\n  letsencrypt: { email: ops@example.com }\n";

pub(super) fn run(top_extra: &str, services: &str) -> (tempfile::TempDir, Outcome) {
    let dir = tempfile::tempdir().unwrap();
    let text = format!("{TOP}{top_extra}services:\n{services}");
    let o = load_in(dir.path(), &text);
    (dir, o)
}

pub(super) fn errs(top_extra: &str, services: &str) -> Vec<String> {
    errors(&run(top_extra, services).1)
}

pub(super) fn warnings(o: &Outcome) -> Vec<String> {
    o.report
        .diagnostics
        .iter()
        .filter(|d| !d.is_error())
        .map(|d| d.message.clone())
        .collect()
}

fn svc(name: &str, web: &str) -> String {
    format!("  {name}:\n    client: home\n    target: 8080\n    web: {web}\n")
}

#[test]
fn a_site_with_every_p3_key_written_right_validates() {
    let s = svc(
        "api",
        "{ hosts: [example.com], paths: [/api, /v2], app_path: /v1, redirect_from: [www.example.com], redirects: { /old: /new, /feed: { to: \"https://feeds.example.net/blog\", permanent: true } }, maintenance: false, hsts: true, compress: true, max_body_size: 8GB, response_header_timeout_secs: 300, request_headers: { Host: app.internal, X-Debug: \"\" }, response_headers: { Access-Control-Allow-Origin: \"https://example.com\" } }",
    ) + &svc("site", "example.com");
    assert_eq!(errs("", &s), Vec::<String>::new());
}

#[test]
fn http_keys_on_a_passthrough_service_are_refused_each_by_name() {
    let s = svc(
        "nas",
        "{ hosts: [nas.example.com], tls: passthrough, paths: [/a], maintenance: true, hsts: true, compress: true, response_headers: { X-A: b } }",
    );
    let found = errs("", &s);
    for key in [
        "paths",
        "maintenance",
        "hsts",
        "compress",
        "response_headers",
    ] {
        let want = format!(
            "services.nas.web.{key}: burrow cannot see the requests of a passthrough service (tls: passthrough); remove {key}, or let burrow handle TLS"
        );
        assert!(found.contains(&want), "{key}: {found:#?}");
    }
    assert_eq!(found.len(), 5, "{found:#?}");
}

#[test]
fn paths_and_app_path_are_checked_after_normalization() {
    let one = |web: &str| errs("", &svc("api", web));
    assert_eq!(
        one("{ hosts: [example.com], paths: [\"/api/*\"] }"),
        [
            "services.api.web.paths: path \"/api/*\" must start with / and hold no ?, # or *, like /api"
        ]
    );
    assert_eq!(
        one("{ hosts: [example.com], paths: [/api, \"/%61pi/\"] }"),
        ["services.api.web.paths lists \"/api\" twice"]
    );
    assert_eq!(
        one("{ hosts: [example.com], app_path: / }"),
        [
            "services.api.web.app_path: app_path needs paths: it replaces the matched path, like paths: [/api] with app_path: /"
        ]
    );
    assert_eq!(
        one("{ hosts: [example.com], paths: [/api], app_path: v1 }"),
        ["services.api.web.app_path: \"v1\" must start with /, like /v1"]
    );
}

#[test]
fn a_redirect_goes_to_a_full_url_or_a_path_never_a_protocol_relative_one() {
    for bad in [
        "new.example.com",
        "//evil.example.com",
        "ftp://x.example.com",
    ] {
        let web = format!("{{ hosts: [example.com], redirects: {{ /old: \"{bad}\" }} }}");
        assert_eq!(
            errs("", &svc("blog", &web)),
            [format!(
                "redirect target \"{bad}\" must be a full URL (https://...) or a path (/...)"
            )],
            "{bad}"
        );
    }
}

#[test]
fn redirect_from_needs_a_plain_first_host_and_names_of_its_own() {
    assert_eq!(
        errs(
            "  certificates: [{ hosts: [\"*.apps.example.com\"], cert: w.pem, key: w.key }]\n",
            &svc(
                "apps",
                "{ hosts: [\"*.apps.example.com\"], redirect_from: [apps.example.net] }"
            )
        )
        .into_iter()
        .filter(|e| e.contains("redirect_from"))
        .collect::<Vec<_>>(),
        ["services.apps.web.redirect_from: the first host must be a plain name to redirect to"]
    );
    let s = svc(
        "blog",
        "{ hosts: [example.com], redirect_from: [www.example.com] }",
    ) + &svc("www", "www.example.com");
    assert_eq!(
        errs("", &s),
        [
            "services.blog.web.redirect_from: \"www.example.com\" is also a host of service \"www\"; a name either serves one site or redirects to it"
        ]
    );
}

#[test]
fn services_share_a_host_by_paths_but_never_one_path_or_two_certificates() {
    let ok = svc("site", "example.com") + &svc("api", "{ hosts: [example.com], paths: [/api] }");
    assert_eq!(errs("", &ok), Vec::<String>::new());
    let twice = svc("blog", "example.com") + &svc("shop", "example.com");
    assert_eq!(
        errs("", &twice),
        [
            "host \"example.com\" is used by services \"blog\" and \"shop\"; give one of them other paths, like paths: [/api]"
        ]
    );
    let same = svc("a", "{ hosts: [example.com], paths: [/api] }")
        + &svc("b", "{ hosts: [example.com], paths: [\"/api/\"] }");
    assert_eq!(
        errs("", &same),
        [
            "host \"example.com\" is used by services \"a\" and \"b\"; give one of them other paths, like paths: [/api]"
        ]
    );
    let tls =
        svc("a", "example.com") + &svc("b", "{ hosts: [example.com], paths: [/api], tls: off }");
    let found = errs("", &tls);
    assert!(
        found.contains(&"host \"example.com\": services \"a\" and \"b\" share it but choose different certificates; set tls for it in one service only".to_string()),
        "{found:#?}"
    );
}

#[test]
fn header_rules_need_valid_names_and_values_and_leave_http_framing_alone() {
    let cases = [
        (
            "\"Bad Name\": x",
            "\"Bad Name\" is not a header name; a name is letters, digits and any of ! # $ % & ' * + - . ^ _ ` | ~, with no spaces, like X-Frame-Options",
        ),
        (
            "Transfer-Encoding: chunked",
            "\"Transfer-Encoding\" belongs to HTTP's own framing, which burrow handles; it cannot be set",
        ),
        (
            "X-Bell: \"a\u{7}b\"",
            "the value of \"X-Bell\" holds characters a header cannot carry",
        ),
    ];
    for (rule, want) in cases {
        let site = format!("{{ hosts: [example.com], request_headers: {{ {rule} }} }}");
        assert_eq!(
            errs("", &svc("blog", &site)),
            [format!("services.blog.web.request_headers: {want}")],
            "{rule}"
        );
        let top = format!("  response_headers: {{ {rule} }}\n");
        assert_eq!(
            errs(&top, &svc("blog", "example.com")),
            [format!("web.response_headers: {want}")],
            "{rule}"
        );
    }
}

#[test]
fn a_size_needs_a_number_and_a_unit_at_both_levels() {
    assert_eq!(
        errs("  max_body_size: lots\n", &svc("blog", "example.com")),
        ["web.max_body_size: \"lots\" does not start with a positive number"]
    );
    assert_eq!(
        errs(
            "",
            &svc("blog", "{ hosts: [example.com], max_body_size: 10parsecs }")
        ),
        [
            "services.blog.web.max_body_size: \"parsecs\" is not a size unit; use B, KB, MB, GB or TB, like 100MB"
        ]
    );
}

#[test]
fn hsts_needs_https() {
    assert_eq!(
        errs(
            "",
            &svc(
                "wiki",
                "{ hosts: [wiki.example.com], tls: off, hsts: true }"
            )
        ),
        ["services.wiki.web.hsts: a site with tls: off has no HTTPS; remove hsts"]
    );
    let dir = tempfile::tempdir().unwrap();
    let o = load_in(
        dir.path(),
        &format!(
            "listen: 7835\nweb: {{ http: 80 }}\nservices:\n{}",
            svc("wiki", "{ hosts: [wiki.example.com], hsts: true }")
        ),
    );
    assert_eq!(
        errors(&o),
        [
            "services.wiki.web.hsts: hsts needs web.https; without web.https every site is plain HTTP. Add https: 443, or remove hsts"
        ]
    );
}

#[test]
fn error_pages_must_be_a_folder_and_hold_a_known_page() {
    let (dir, o) = run("  error_pages: pages\n", &svc("blog", "example.com"));
    drop(dir);
    let found = errors(&o);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].starts_with("web.error_pages: cannot read the folder pages: "),
        "{found:#?}"
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("pages")).unwrap();
    std::fs::write(dir.path().join("pages/other.html"), "x").unwrap();
    let text = format!(
        "{TOP}services:\n{}",
        svc("blog", "{ hosts: [example.com], error_pages: pages }")
    );
    let o = load_in(dir.path(), &text);
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
    assert!(
        warnings(&o).iter().any(|w| w.starts_with(
            "services.blog.web.error_pages: pages holds none of 404.html, 413.html, 429.html"
        )),
        "{:#?}",
        warnings(&o)
    );
    std::fs::write(dir.path().join("pages/413.html"), "too big").unwrap();
    let o = load_in(dir.path(), &text);
    assert!(
        !warnings(&o).iter().any(|w| w.contains("error_pages")),
        "{:#?}",
        warnings(&o)
    );
}

#[test]
fn target_tls_reads_its_ca_and_says_what_is_not_checked() {
    let (_d, o) = run(
        "",
        "  erp:\n    client: home\n    target: 192.168.1.10:443\n    web: { hosts: [erp.example.com], target_tls: { ca: certs/corp-ca.pem } }\n",
    );
    let found = errors(&o);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].starts_with("services.erp.web.target_tls.ca: cannot read certs/corp-ca.pem: "),
        "{found:#?}"
    );
    assert!(warnings(&o).contains(&"target \"192.168.1.10:443\" is an IP address; the app's certificate must name that IP".to_string()), "{:#?}", warnings(&o));
    let (_d, o) = run(
        "",
        &svc(
            "pve",
            "{ hosts: [pve.example.com], target_tls: { insecure_skip_verify: true } }",
        ),
    );
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
    assert!(warnings(&o).contains(&"services.pve: the app's certificate is not checked (insecure_skip_verify); use ca: for apps with a real certificate".to_string()), "{:#?}", warnings(&o));
}

#[test]
fn paths_differing_only_by_letter_case_are_one_path() {
    let one = |web: &str| errs("", &svc("api", web));
    assert_eq!(
        one("{ hosts: [example.com], paths: [/api, /API/] }"),
        [
            "services.api.web.paths: \"/api\" and \"/API\" are the same path; paths match whatever the letter case"
        ]
    );
    assert_eq!(
        one("{ hosts: [example.com], redirects: { /Old: /new, /old/: /x } }"),
        ["services.api.web.redirects: \"/Old\" and \"/old/\" are the same path /old"]
    );
    let two = svc("a", "{ hosts: [example.com], paths: [/api] }")
        + &svc("b", "{ hosts: [example.com], paths: [/Api] }");
    assert_eq!(
        errs("", &two),
        [
            "host \"example.com\" is used by services \"a\" and \"b\"; give one of them other paths, like paths: [/api]"
        ]
    );
}

#[test]
fn every_header_burrow_strips_or_frames_is_refused_in_a_rule() {
    for name in burrow_syntax::web::headers::NOT_SETTABLE {
        let site = format!("{{ hosts: [example.com], request_headers: {{ {name}: x }} }}");
        assert_eq!(
            errs("", &svc("blog", &site)),
            [format!(
                "services.blog.web.request_headers: \"{name}\" belongs to HTTP's own framing, which burrow handles; it cannot be set"
            )],
            "{name}"
        );
    }
}
