use super::tests::{errors, load_in};
use super::tests_site_keys::{TOP, errs, run, warnings};

const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$3Zrnt9ZV6D9A/Ed3Je/Hww$lg26ncNCHaoX2amIl0p8mWtR1BCyscvYzi1EmeCfbvg";

fn svc(name: &str, web: &str) -> String {
    format!("  {name}:\n    client: home\n    target: 8080\n    web: {web}\n")
}

#[test]
fn password_hashes_are_argon2id_or_bcrypt_within_the_cost_caps() {
    let one = |user: &str, hash: &str| {
        errs(
            "",
            &svc(
                "status",
                &format!(
                    "{{ hosts: [status.example.com], basic_auth: {{ \"{user}\": \"{hash}\" }} }}"
                ),
            ),
        )
    };
    assert_eq!(one("alice", HASH), Vec::<String>::new());
    assert_eq!(
        one("a:b", HASH),
        ["services.status.web.basic_auth: user name \"a:b\" must not be empty or hold \":\""]
    );
    assert_eq!(
        one("bob", "hunter2"),
        [
            "services.status.web.basic_auth: the hash for \"bob\" is not argon2id or bcrypt; make one with burrow passwd bob"
        ]
    );
    assert_eq!(
        one(
            "bob",
            "$2x$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy"
        ),
        [
            "services.status.web.basic_auth: the hash for \"bob\" is bcrypt $2x$, an old variant with a known bug; make a new one with burrow passwd bob"
        ]
    );
    assert_eq!(
        one("bob", &HASH.replacen("$argon2id$", "$argon2d$", 1)),
        [
            "services.status.web.basic_auth: the hash for \"bob\" is argon2d; only argon2id is accepted"
        ]
    );
    assert_eq!(
        one("bob", &HASH.replacen("m=19456", "m=4194304", 1)),
        [
            "services.status.web.basic_auth: the hash for \"bob\" asks for more work than burrow allows (argon2 memory 4194304 KiB, at most 65536 KiB); make one with burrow passwd bob"
        ]
    );
}

#[test]
fn passwords_over_plain_http_are_a_warning_and_passthrough_refuses_them() {
    let (_d, o) = run(
        "",
        &svc(
            "wiki",
            &format!(
                "{{ hosts: [wiki.example.com], tls: off, basic_auth: {{ alice: \"{HASH}\" }} }}"
            ),
        ),
    );
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
    assert!(warnings(&o).contains(&"services.wiki.web.basic_auth: passwords are sent unencrypted over plain HTTP; use HTTPS, or keep the site on a trusted network".to_string()), "{:#?}", warnings(&o));
    assert_eq!(
        errs(
            "",
            &svc(
                "nas",
                &format!(
                    "{{ hosts: [nas.example.com], tls: passthrough, basic_auth: {{ alice: \"{HASH}\" }} }}"
                )
            )
        ),
        [
            "services.nas.web.basic_auth: burrow cannot see the requests of a passthrough service (tls: passthrough); remove basic_auth, or let burrow handle TLS"
        ]
    );
}

#[test]
fn device_certificates_need_https_their_files_and_a_name_without_the_tunnel() {
    assert_eq!(
        errs(
            "",
            &svc(
                "lan",
                "{ hosts: [lan.example.com], tls: off, client_auth: { ca: ca.pem } }"
            )
        ),
        ["services.lan.web.client_auth: a site with tls: off has no HTTPS; remove client_auth"]
    );
    let found = errs(
        "",
        &svc(
            "grafana",
            "{ hosts: [grafana.example.com], client_auth: { ca: certs/devices.pem } }",
        ),
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .starts_with("services.grafana.web.client_auth.ca: cannot read certs/devices.pem: "),
        "{found:#?}"
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ca.pem"), "x").unwrap();
    let text = format!(
        "websocket: {{ listen: 443, host: example.com, path: /burrow }}\n{TOP}services:\n{}",
        svc(
            "blog",
            "{ hosts: [example.com], client_auth: { ca: ca.pem } }"
        )
    );
    let o = load_in(dir.path(), &text.replace("listen: 7835\n", ""));
    assert!(
        errors(&o).contains(&"services.blog.web.client_auth: \"example.com\" also carries the tunnel (websocket.host, by path); burrow clients have no device certificate, so they could not connect. Give the tunnel its own name, or remove client_auth".to_string()),
        "{:#?}",
        errors(&o)
    );
    let shared = svc("a", "{ hosts: [example.com], client_auth: { ca: ca.pem } }")
        + &svc("b", "{ hosts: [example.com], paths: [/api] }");
    let (_d, o) = run("", &shared);
    assert!(
        errors(&o).contains(&"host \"example.com\": services share it but name different client_auth CAs; device certificates are checked per host name".to_string()),
        "{:#?}",
        errors(&o)
    );
}

#[test]
fn device_certificates_refuse_https_redirect_false_on_the_site_or_for_all_sites() {
    let site = |extra: &str| {
        svc(
            "nas",
            &format!("{{ hosts: [nas.example.com], client_auth: {{ ca: ca.pem }}{extra} }}"),
        )
    };
    let refusal = |from: &str| {
        format!(
            "services.nas.web.client_auth: {from} would serve this site over plain http:// too, where no device certificate can be asked; remove https_redirect: false, or remove client_auth"
        )
    };
    let own = errs("", &site(", https_redirect: false"));
    assert!(own.contains(&refusal("https_redirect: false")), "{own:#?}");
    let all = errs("  https_redirect: false\n", &site(""));
    assert!(
        all.contains(&refusal("web.https_redirect: false")),
        "{all:#?}"
    );
    for (top, extra) in [
        ("", ""),
        ("", ", https_redirect: true"),
        ("  https_redirect: false\n", ", https_redirect: true"),
    ] {
        let found = errs(top, &site(extra));
        assert!(
            !found.iter().any(|e| e.contains("would serve this site")),
            "{top}{extra}: {found:#?}"
        );
    }
    let open = errs(
        "",
        &svc(
            "blog",
            "{ hosts: [blog.example.com], https_redirect: false }",
        ),
    );
    assert_eq!(open, Vec::<String>::new());
}

#[test]
fn a_device_ca_must_be_a_ca_certificate() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ca.pem"), "not a certificate").unwrap();
    let text = format!(
        "{TOP}services:\n{}",
        svc(
            "grafana",
            "{ hosts: [grafana.example.com], client_auth: { ca: ca.pem } }"
        )
    );
    let o = load_in(dir.path(), &text);
    assert!(
        errors(&o)
            .contains(&"services.grafana.web.client_auth.ca: is not a CA certificate".to_string()),
        "{:#?}",
        errors(&o)
    );
}

fn argon2(alg: argon2::Algorithm, m: u32, t: u32, p: u32) -> String {
    use argon2::password_hash::PasswordHasher;
    let params = argon2::Params::new(m, t, p, None).unwrap();
    argon2::Argon2::new(alg, argon2::Version::V0x13, params)
        .hash_password(b"pw")
        .unwrap()
        .to_string()
}

fn bcrypt_as(v: bcrypt::Version, cost: u32) -> String {
    bcrypt::hash_with_result("pw", cost)
        .unwrap()
        .format_for_version(v)
}

#[test]
fn validate_accepts_a_hash_exactly_when_the_server_lets_its_password_in() {
    use argon2::Algorithm::{Argon2d, Argon2i, Argon2id};
    let default = burrow_web::auth::hash_password(b"pw");
    let b4 = bcrypt_as(bcrypt::Version::TwoB, 4);
    let table: Vec<(String, bool)> = vec![
        (default.clone(), true),
        (default.replacen("v=19$", "", 1), true),
        (format!("{default} "), true),
        (argon2(Argon2id, 8, 1, 1), true),
        (argon2(Argon2id, 65536, 10, 4), true),
        (
            "$argon2id$v=19$m=1,t=1,p=1$c2FsdHNhbHQ$aGFzaGhhc2g".into(),
            false,
        ),
        (argon2(Argon2id, 65537, 1, 1), false),
        (argon2(Argon2id, 64, 11, 1), false),
        (argon2(Argon2id, 64, 1, 5), false),
        (argon2(Argon2i, 64, 1, 1), false),
        (argon2(Argon2d, 64, 1, 1), false),
        (bcrypt_as(bcrypt::Version::TwoA, 4), true),
        (b4.clone(), true),
        (bcrypt_as(bcrypt::Version::TwoY, 4), true),
        (bcrypt_as(bcrypt::Version::TwoX, 4), false),
        (format!("{}15{}", &b4[..4], &b4[6..]), false),
        (format!("{}03{}", &b4[..4], &b4[6..]), false),
        ("hunter2".into(), false),
        ("{SHA}abc".into(), false),
    ];
    for (hash, want) in table {
        let config = svc(
            "status",
            &format!("{{ hosts: [status.example.com], basic_auth: {{ bob: \"{hash}\" }} }}"),
        );
        let validated = errs("", &config).is_empty();
        let logs_in = burrow_web::auth::verify_hash(b"pw", &hash);
        assert_eq!(validated, want, "validate on {hash:?}");
        assert_eq!(logs_in, want, "the server on {hash:?}");
    }
}

#[test]
fn a_public_port_beside_a_login_says_it_skips_the_login() {
    let site = |extra: &str| {
        format!(
            "  nas:\n    client: home\n    target: 8080\n{extra}    web: {{ hosts: [nas.example.com], basic_auth: {{ alice: \"{HASH}\" }} }}\n"
        )
    };
    let top = "";
    let found = |extra: &str| {
        let (_dir, o) = run(top, &site(extra));
        assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
        warnings(&o)
            .into_iter()
            .filter(|w| w.contains("reaches the app directly"))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        found("    bind: 9000\n"),
        [
            "services.nas.bind: bind 9000 reaches the app directly, without the website's basic_auth; remove bind, or limit it with allow_from"
        ]
    );
    assert!(found("").is_empty(), "no bind, no warning");
    assert!(found("    bind: 9000\n    allow_from: [10.0.0.0/8]\n").is_empty());
    assert!(found("    bind: 127.0.0.1:9000\n").is_empty());
    let (_dir, o) = run(top, &site("    bind: 9000\n"));
    let d = o
        .report
        .diagnostics
        .iter()
        .find(|d| d.code == burrow_admin_proto::codes::PUBLIC_PORT_SKIPS_LOGIN)
        .expect("the code");
    assert!(!d.is_error());
}
