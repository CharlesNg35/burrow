use super::tests::load;

fn found(rest: &str, code: &str) -> Vec<String> {
    load(rest)
        .report
        .diagnostics
        .iter()
        .filter(|d| d.code == code)
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn the_web_timeouts_are_at_least_one_second() {
    for key in ["request_header_timeout_secs", "idle_timeout_secs"] {
        let text = format!("web:\n  https: 443\n  {key}: 0\n");
        assert_eq!(
            found(&text, "out-of-range"),
            [format!("\"{key}\" in \"web\" must be at least 1, found 0")],
            "{key}"
        );
        let ok = format!("web:\n  https: 443\n  {key}: 1\n");
        assert!(found(&ok, "out-of-range").is_empty(), "{key}");
    }
}

fn profile_of(le: serde_json::Value) -> Option<String> {
    let cfg: burrow_config::ServerConfig =
        serde_json::from_value(serde_json::json!({"web": {"https": 443, "letsencrypt": le}}))
            .unwrap();
    let web = super::fill::settings(
        &cfg,
        (Some("[::]:443".parse().unwrap()), None),
        Default::default(),
        None,
        std::path::Path::new("."),
    )
    .unwrap();
    web.letsencrypt.unwrap().profile
}

#[test]
fn the_letsencrypt_profile_is_passed_on_as_written() {
    let email = "ops@example.com";
    assert_eq!(
        profile_of(serde_json::json!({"email": email, "profile": " shortlived "})).as_deref(),
        Some("shortlived")
    );
    assert_eq!(profile_of(serde_json::json!({"email": email})), None);
    assert_eq!(
        profile_of(serde_json::json!({"email": email, "profile": ""})),
        None
    );
    assert!(
        found(
            "web:\n  https: 443\n  letsencrypt: {email: ops@example.com, profile: anything}\n",
            "invalid-profile"
        )
        .is_empty(),
        "the CA decides which profiles exist"
    );
}

#[test]
fn a_plain_site_needs_the_http_port() {
    let site = |ports: &str| {
        format!(
            "web: {{ {ports} }}\nservices:\n  wiki:\n    client: home\n    target: 3000\n    web: {{ hosts: [wiki.example.com], tls: off }}\n"
        )
    };
    assert_eq!(
        found(&site("https: 443"), "plain-needs-http"),
        ["services.wiki.web.tls: off serves plain HTTP, but web.http is not set; add http: 80"]
    );
    assert!(found(&site("https: 443, http: 80"), "plain-needs-http").is_empty());
    assert!(found(&site("http: 80"), "plain-needs-http").is_empty());
}

fn site_with(ports: &str, web: &str) -> String {
    format!(
        "web: {{ {ports} }}\nservices:\n  nas:\n    client: home\n    target: 443\n    web: {web}\n"
    )
}

#[test]
fn a_redirect_choice_that_cannot_work_is_refused() {
    let pass = site_with(
        "https: 443, http: 80",
        "{hosts: [nas.example.com], tls: passthrough, https_redirect: false}",
    );
    assert_eq!(
        found(&pass, "redirect-passthrough"),
        [
            "services.nas.web.https_redirect: false: burrow cannot serve plain HTTP for a passthrough service (the app keeps its own TLS); only the redirect is possible"
        ]
    );
    let plain = site_with(
        "https: 443, http: 80",
        "{hosts: [nas.example.com], tls: off, https_redirect: true}",
    );
    assert_eq!(found(&plain, "redirect-plain").len(), 1);
    let plain_by_ports = site_with(
        "http: 80",
        "{hosts: [nas.example.com], https_redirect: true}",
    );
    assert_eq!(found(&plain_by_ports, "redirect-plain").len(), 1);
    let fine = site_with(
        "https: 443, http: 80",
        "{hosts: [nas.example.com], tls: off, https_redirect: false}",
    );
    assert!(found(&fine, "redirect-plain").is_empty());
}

#[test]
fn an_https_site_that_also_answers_plain_http_needs_the_http_port() {
    let own = site_with(
        "https: 443",
        "{hosts: [nas.example.com], tls: auto, https_redirect: false}",
    );
    assert_eq!(
        found(&own, "redirect-needs-http"),
        [
            "services.nas.web.https_redirect: false serves plain HTTP too, but web.http is not set; add http: 80"
        ]
    );
    let top = site_with("https: 443, https_redirect: false", "nas.example.com");
    assert_eq!(found(&top, "redirect-needs-http").len(), 1);
    let ok = site_with(
        "https: 443, http: 80, https_redirect: false",
        "nas.example.com",
    );
    assert!(found(&ok, "redirect-needs-http").is_empty());
}

#[test]
fn a_single_label_name_is_for_plain_http_sites_only() {
    let site = |ports: &str, tls: &str| {
        format!(
            "listen: 7835\nweb: {{ {ports} }}\nservices:\n  nas:\n    client: home\n    target: 5000\n    web: {{ hosts: [nas]{tls} }}\n"
        )
    };
    assert!(super::tests::errors(&load(&site("http: 80", ""))).is_empty());
    assert!(super::tests::errors(&load(&site("https: 443, http: 80", ", tls: off"))).is_empty());
    for (ports, tls) in [
        ("https: 443, http: 80", ""),
        ("https: 443", ", tls: passthrough"),
    ] {
        assert_eq!(
            found(&site(ports, tls), "single-label-host"),
            [
                "services.nas.web.hosts: \"nas\" is a single-label name; HTTPS finds sites by their full name, so use a full name like nas.home.example.com, or set tls: off to serve it as plain HTTP"
            ],
            "{ports}{tls}"
        );
    }
}
