use super::*;

#[test]
fn a_missing_field_reads_as_required_not_as_the_validate_text() {
    let raw = "missing field \"target\" in service \"pool\"";
    assert_eq!(missing_field(MISSING_FIELD, raw), Some("target"));
    assert_eq!(missing_field("other", raw), None);
    assert_eq!(shown(MISSING_FIELD, raw, &[]), "target is required");
    assert_eq!(required("Main target"), "Main target is required");
    let label = "Main target";
    assert_eq!(beside_label(label, &required(label)), "required");
    assert_eq!(
        beside_label("Weight", "Weight must be 1 to 1000"),
        "must be 1 to 1000"
    );
    assert_eq!(beside_label("Weight", "Weights add up"), "Weights add up");
    assert_eq!(beside_label("Bind", "port taken"), "port taken");
    assert_eq!(capital("a weight"), "A weight");
    assert_eq!(capital(""), "");
    assert_eq!(version_name(Some("15:47:12")), "version of 15:47:12");
    assert_eq!(version_name(None), "version (time unknown)");
    assert_eq!(count_of(1, "field"), "1 field");
    assert_eq!(count_of(2, "field"), "2 fields");
    assert_eq!(count_of(0, "field"), "0 fields");
    assert_eq!(used_by_sites(1), "Used by 1 site: ");
    assert_eq!(result_words("kept"), "kept in history");
    assert_eq!(
        result_words("unchanged"),
        "no change: the file already matched"
    );
    assert_eq!(
        diff_head(11, 3, true),
        "11 added, 3 removed · secrets hidden"
    );
    assert_eq!(
        rewritten("Services", "a list of mappings"),
        "burrow rewrites Services in server.yaml in its own layout, because the new value cannot keep the old short form. Comments in that part are lost."
    );
    use burrow_admin_proto::schema::Applies;
    assert_eq!(applies_label(&Applies::Live), Some("applies at once"));
    assert_eq!(
        applies_label(&Applies::Reload),
        Some("applies to new connections")
    );
    assert_eq!(applies_label(&Applies::Restart), Some("needs a restart"));
    let d: burrow_admin_proto::diag::Diag = burrow_admin_proto::serde_json::from_value(
        burrow_admin_proto::serde_json::json!({
            "severity": "error",
            "code": "out-of-range",
            "message": "\"weight\" in \"services.pool.clients[0]\" must be between 1 and 1000, found 0",
            "label": "must be between 1 and 1000",
            "path": ["services", "pool", "clients", "0", "weight"]
        }),
    )
    .unwrap();
    assert_eq!(diag_text(&d), "Weight: 1 to 1000");
    let d: burrow_admin_proto::diag::Diag =
        burrow_admin_proto::serde_json::from_value(burrow_admin_proto::serde_json::json!({
            "severity": "error",
            "code": "missing-field",
            "message": "missing field \"target\" in service \"pool\"",
            "path": ["services", "pool"]
        }))
        .unwrap();
    assert_eq!(diag_text(&d), "Main target is required");
}

#[test]
fn a_read_admin_is_told_once_in_one_wording_why_a_change_is_off() {
    assert_eq!(
        needs_write_admin(WriteAction::Add, None),
        "Adding needs write admin."
    );
    assert_eq!(
        needs_write_admin(WriteAction::Edit, Some("alice")),
        "Editing needs write admin. alice has read."
    );
    assert_eq!(
        needs_write_admin(WriteAction::Edit, Some("")),
        "Editing needs write admin."
    );
}

#[test]
fn a_name_let_s_encrypt_cannot_check_says_which_ports_it_needs() {
    for m in [
        "blog.example.com (service \"blog\") cannot be checked by Let's Encrypt: open http: 80, ...",
        "websocket.host \"blog.example.com\" cannot be checked: open http: 80, ...",
    ] {
        let shown = shown(NO_CHECK, m, &["blog.example.com".into()]);
        assert!(
            shown.starts_with("Let's Encrypt cannot check blog.example.com:"),
            "{shown}"
        );
    }
}

#[test]
fn a_website_with_the_web_proxy_off_says_where_to_turn_it_on() {
    let m = shown(
        WEB_OFF,
        "services.blog.web: the web proxy is off; add a top-level web: with its ports, like web: {https: 443, http: 80}",
        &[],
    );
    assert!(
        m.contains("Settings, Web proxy") && !m.contains("web:"),
        "{m}"
    );
}

#[test]
fn a_plain_http_website_is_named_in_its_warning() {
    let item = |name: &str| {
        shown(
            PLAIN_HTTP,
            &format!(
                "services.{name}: served as plain HTTP; anyone on the path can read and change its traffic"
            ),
            &[name.to_string()],
        )
    };
    assert_eq!(
        item("wiki"),
        "wiki is served as plain HTTP: anyone on the path can read and change its traffic"
    );
    assert_ne!(item("wiki"), item("dashboard"));
}

#[test]
fn a_name_without_a_certificate_says_how_to_get_one_in_the_ui() {
    let m = shown(
        NO_CERTIFICATE,
        "host \"nas.example.com\" (service \"nas\") has no certificate; add web.letsencrypt for automatic certificates, tls: {cert, key} on service \"nas\", or an entry in web.certificates whose hosts lists it",
        &["nas.example.com".into()],
    );
    assert!(
        m.starts_with("nas.example.com has no certificate: turn on Let's Encrypt"),
        "{m}"
    );
    assert!(!m.contains("web.") && !m.contains("tls:"), "{m}");
    let w = shown(
        WILDCARD_NEEDS_DNS,
        "services.apps.web.hosts: \"*.apps.example.com\" needs DNS-01 (...)",
        &["*.apps.example.com".into()],
    );
    assert!(
        w.contains("*.apps.example.com only through a DNS check"),
        "{w}"
    );
}

#[test]
fn the_redirect_choices_say_what_to_do_in_plain_words() {
    let too = shown(
        REDIRECT_NEEDS_HTTP,
        "services.nas.web.https_redirect: false serves plain HTTP too, but web.http is not set; add http: 80",
        &[],
    );
    assert!(too.starts_with("answering plain HTTP too"), "{too}");
    for code in [PASSTHROUGH_FIXED, REDIRECT_PLAIN] {
        let m = shown(code, "services.nas.web.https_redirect: ...", &[]);
        assert!(
            !m.contains("https_redirect") && !m.contains("services."),
            "{m}"
        );
    }
}

#[test]
fn a_tcp_only_field_on_a_udp_service_says_its_label_and_the_fix() {
    let cases = [
        (
            PROXY_PROTOCOL_TCP_ONLY,
            "service \"web\" is udp, but proxy_protocol only works for tcp services",
            "PROXY protocol only works for TCP services; set Protocol to TCP, or clear it",
        ),
        (
            MAX_CONNECTIONS_TCP_ONLY,
            "service \"web\" is udp, but max_connections only works for tcp services; for UDP, udp.max_peers limits the peers of each port",
            "Maximum connections only works for TCP services; set Protocol to TCP, or clear it (for UDP, Maximum UDP senders per port under Settings limits the senders of each port)",
        ),
        (
            BALANCE_TCP_ONLY,
            "service \"web\" is udp, but mode balance only works for tcp services",
            "Balance only works for TCP services; set Protocol to TCP, or choose Failover",
        ),
        (
            BALANCE_TCP_ONLY,
            "service \"web\" is udp, but mode ip_hash only works for tcp services",
            "IP hash only works for TCP services; set Protocol to TCP, or choose Failover",
        ),
        (
            BALANCE_TCP_ONLY,
            "service \"web\" is udp, but mode least_connections only works for tcp services",
            "Least connections only works for TCP services; set Protocol to TCP, or choose Failover",
        ),
        (
            HEALTH_CHECK_TCP_ONLY,
            "service \"web\" is udp, but health_check only works for tcp services",
            "Health check only works for TCP services; set Protocol to TCP, or turn it off",
        ),
    ];
    for (code, cli, ui) in cases {
        assert_eq!(shown(code, cli, &[]), ui, "{code}");
        assert!(
            !shown(code, cli, &[]).contains('_'),
            "{code}: no config keys"
        );
    }
}

#[test]
fn a_message_loses_its_config_key_path_but_keeps_its_words() {
    assert_eq!(
        shown(
            "invalid-host",
            "services.nas.web.hosts: \"x\" is not a host name",
            &[],
        ),
        "\"x\" is not a host name"
    );
    assert_eq!(
        shown(
            "tunnel-host",
            "websocket.host: \"*.a.b\" is a wildcard",
            &[]
        ),
        "\"*.a.b\" is a wildcard"
    );
    assert_eq!(
        shown(
            "duplicate-host",
            "host \"a.example.com\" is used by services \"x\" and \"y\"",
            &[],
        ),
        "host \"a.example.com\" is used by services \"x\" and \"y\""
    );
    assert_eq!(
        shown("x", "invalid bind address for service \"web\": bad", &[]),
        "invalid bind address for service \"web\": bad"
    );
    assert_eq!(
        shown(
            WEB_TCP_ONLY,
            "service \"dns\" is udp, but web only works for tcp services; remove web:, or set protocol: tcp",
            &[],
        ),
        "a website only works for TCP services; set Protocol to TCP, or remove the website"
    );
    assert_eq!(
        shown(
            PLAIN_NEEDS_HTTP,
            "services.wiki.web.tls: off serves plain HTTP, but web.http is not set; add http: 80",
            &[],
        ),
        "plain HTTP needs the web proxy's HTTP port, which is not set; set the HTTP port (80) in Settings, Web proxy, or choose another HTTPS option"
    );
    assert_eq!(
        help_shown(
            NO_HTTP,
            Some("add http: 80 so http:// addresses are sent to https://")
        )
        .as_deref(),
        Some(
            "Set the HTTP port (80) in Settings, Web proxy so http:// addresses are sent to https://"
        ),
        "the UI says where, not the config syntax"
    );
    assert_eq!(
        field_for(NO_HTTP),
        Some(vec!["web".to_string(), "http".to_string()])
    );
    assert_eq!(help_shown("other", Some("x")).as_deref(), Some("x"));
}

#[test]
fn needs_attention_speaks_plain_english_without_codes_or_yaml() {
    let who = ["plain".to_string()];
    assert_eq!(
        shown(
            UNUSED_VISITOR,
            "visitor \"plain\" cannot reach anything",
            &who
        ),
        "Visitor plain can't reach anything"
    );
    assert_eq!(
        help_shown(
            UNUSED_VISITOR,
            Some("add it to a service, for example visitors: [plain]")
        )
        .as_deref(),
        Some("Add it to a service's Visitors, or give it direct access to a client.")
    );
    assert_eq!(
        shown(NO_HTTP, "web.http is not set: x", &[]),
        "No HTTP port: http:// addresses get no answer"
    );
    assert_eq!(help_shown("other", Some("set web.http: 80 there")), None);
    assert_eq!(
        help_shown("other", Some("give it reach: { CLIENT: any }")),
        None
    );
    assert_eq!(
        help_shown("other", Some("pick a client")).as_deref(),
        Some("pick a client")
    );
}

#[test]
fn a_failed_check_says_whether_the_server_refused_or_did_not_answer() {
    use burrow_admin_proto::msg::ErrorCode;
    for code in [
        ErrorCode::LockedField,
        ErrorCode::BadRequest,
        ErrorCode::Invalid,
    ] {
        assert_eq!(check_failed(code), "The server refused this change");
    }
    for code in [ErrorCode::Busy, ErrorCode::Internal, ErrorCode::Unknown] {
        assert_eq!(check_failed(code), "The server could not check this change");
    }
    assert_eq!(
        locked_here("Heartbeat · Interval"),
        "Heartbeat · Interval is locked. Change this in server.yaml (burrow edit), then restart the server."
    );
    assert_eq!(
        reason_text("too many changes are waiting; try again"),
        "Too many changes are waiting; try again."
    );
    assert_eq!(reason_text("Done."), "Done.");
    assert_eq!(reason_text(""), "");
}

#[test]
fn a_key_led_message_drops_its_key_at_the_field_and_names_the_field_elsewhere() {
    let d = |code: &str, message: &str, path: &[&str]| -> burrow_admin_proto::diag::Diag {
        burrow_admin_proto::serde_json::from_value(burrow_admin_proto::serde_json::json!({
            "severity": "warning",
            "code": code,
            "message": message,
            "path": path,
        }))
        .unwrap()
    };
    let low = d(
        "low-port",
        "web.https: port 80 needs root or CAP_NET_BIND_SERVICE",
        &["web", "https"],
    );
    assert_eq!(
        diag_text(&low),
        "port 80 needs root or CAP_NET_BIND_SERVICE"
    );
    assert_eq!(
        diag_title(&low),
        "HTTPS port: port 80 needs root or CAP_NET_BIND_SERVICE"
    );
    let plain = d(
        "insecure-plaintext",
        "insecure_plaintext: data forwarded between the server and its clients is not encrypted",
        &["insecure_plaintext"],
    );
    assert_eq!(
        diag_text(&plain),
        "data forwarded between the server and its clients is not encrypted"
    );
    let other = d("x", "a plain sentence", &["listen"]);
    assert_eq!(diag_title(&other), "A plain sentence");
}

#[test]
fn an_invalid_entry_says_only_what_is_wrong_beside_its_field() {
    let d: burrow_admin_proto::diag::Diag =
        burrow_admin_proto::serde_json::from_value(burrow_admin_proto::serde_json::json!({
            "severity": "error",
            "code": "invalid-source",
            "message": "invalid allow_from entry for service \"web\": \"80\" is not an IP address or CIDR",
            "path": ["services", "web", "allow_from", "0"]
        }))
        .unwrap();
    assert_eq!(diag_text(&d), "\"80\" is not an IP address or CIDR");
}

#[test]
fn a_port_used_twice_reads_as_a_sentence_without_a_subject() {
    let d: burrow_admin_proto::diag::Diag =
        burrow_admin_proto::serde_json::from_value(burrow_admin_proto::serde_json::json!({
            "severity": "error",
            "code": burrow_admin_proto::codes::DUPLICATE_BIND,
            "message": "web.http 127.0.0.1:80 is also web.https",
            "path": ["web", "http"]
        }))
        .unwrap();
    assert_eq!(
        diag_text(&d),
        "This port is already used by another service or listener; pick another port"
    );
}

#[test]
fn release_notes_link_only_a_strict_release_number_to_the_official_release_page() {
    assert_eq!(
        release_notes("0.2.3"),
        Some((
            "0.2.3".to_string(),
            "https://github.com/CharlesNg35/burrow/releases/tag/v0.2.3".to_string()
        ))
    );
    assert_eq!(
        release_notes("v0.2.4-rc.1").unwrap().1,
        "https://github.com/CharlesNg35/burrow/releases/tag/v0.2.4-rc.1"
    );
    for bad in [
        "",
        "latest",
        "../../x",
        "0.2.3/../../evil",
        "0.2.3+build",
        "99.0.0+/../..",
        "0.2.3?x=1",
        "0.2.3#x",
        "<script>",
        "0.2",
    ] {
        assert_eq!(release_notes(bad), None, "{bad}");
    }
}

#[test]
fn a_field_message_drops_the_check_prefix_that_names_the_field_again() {
    assert_eq!(
        at_field("invalid target for service \"app\": only an IPv6 address goes in brackets"),
        "only an IPv6 address goes in brackets"
    );
    assert_eq!(
        at_field("invalid target for forward \"a: b\": no port"),
        "no port"
    );
    assert_eq!(
        at_field("must be between 1 and 9"),
        "must be between 1 and 9"
    );
    assert_eq!(at_field("invalid CIDR"), "invalid CIDR");
}

#[test]
fn a_long_key_hint_may_break_at_dots_and_underscores() {
    let k = key_hint(&["web".to_string(), "request_header_timeout_secs".to_string()]);
    assert_eq!(
        k.replace('\u{200b}', "|"),
        "web.|request_|header_|timeout_|secs"
    );
}

#[test]
fn an_add_button_keeps_an_acronym_and_lowers_the_rest() {
    assert_eq!(
        add_label("DNS check providers", "DNS provider"),
        "Add DNS provider"
    );
    assert_eq!(
        add_label("Your own certificates", "Certificate"),
        "Add certificate"
    );
    assert_eq!(add_label("Headers", ""), "Add to Headers");
    assert!(dns_guide("cloudflare").is_some_and(|u| u.ends_with("certificates.md#cloudflare")));
    assert_eq!(dns_guide("command"), None);
}

#[test]
fn a_dns_entry_message_drops_its_indexed_key_path() {
    assert_eq!(
        shown(
            "dns-domains",
            "web.letsencrypt.dns[1].domains: \"example.com\" is already listed in DNS provider 1; list it in one entry only",
            &[]
        ),
        "\"example.com\" is already listed in DNS provider 1; list it in one entry only"
    );
}

#[test]
fn a_closed_session_says_what_to_do_for_each_reason() {
    use burrow_admin_proto::msg::revoked;
    assert_eq!(
        super::revoked_words(revoked::SECRET_REMOVED).1,
        "Run burrow admin again with the visitor's new secret."
    );
    assert_eq!(
        super::revoked_words(revoked::LEVEL_CHANGED).0,
        "Your admin level changed"
    );
    assert!(
        super::revoked_words(revoked::SOURCE_NOT_ALLOWED)
            .1
            .contains("Admin only from")
    );
    for gone in [
        revoked::ADMIN_REMOVED,
        revoked::VISITOR_REMOVED,
        "anything else",
    ] {
        assert_eq!(
            super::revoked_words(gone).0,
            "Your admin access was removed"
        );
    }
}
