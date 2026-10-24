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
