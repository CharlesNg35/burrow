use super::tests::{SECRET, errors};
use crate::config::load::load_str_in;
use crate::config::{Format, Outcome, Role};

const KEY32: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

fn load_with(dir: &std::path::Path, dns: &str, services: &str) -> Outcome {
    let services = if services.is_empty() {
        String::new()
    } else {
        format!("services:\n{services}")
    };
    let text = format!(
        "clients:\n  home: {{ secret: \"{SECRET}\" }}\nlisten: 7835\nweb:\n  https: 443\n  letsencrypt:\n    email: ops@example.com\n    dns:\n{dns}{services}"
    );
    let env = |n: &str| match n {
        "CF" => Some("token".to_string()),
        "TSIG" => Some(KEY32.to_string()),
        "SHORT" => Some("c2VjcmV0".to_string()),
        "NOTB64" => Some("not base64!".to_string()),
        _ => None,
    };
    load_str_in(
        dir,
        "server.yaml",
        &text,
        Format::Yaml,
        Some(Role::Server),
        &env,
    )
}

pub(super) fn hook_in(dir: &std::path::Path) -> std::path::PathBuf {
    let hook = dir.join("hook");
    std::fs::write(&hook, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir.to_path_buf()
}

fn errs(dns: &str, services: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    errors(&load_with(dir.path(), dns, services))
}

fn warns(dns: &str, services: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    load_with(dir.path(), dns, services)
        .report
        .diagnostics
        .iter()
        .filter(|d| !d.is_error())
        .map(|d| d.message.clone())
        .collect()
}

const CF: &str = "cloudflare: { token: \"${CF}\" }";

fn entry(domains: &str, rest: &str) -> String {
    format!("      - domains: {domains}\n        {rest}\n")
}

fn site(name: &str, web: &str) -> String {
    format!("  {name}:\n    client: home\n    target: 8080\n    web: {web}\n")
}

#[test]
fn an_entry_needs_its_domains_and_exactly_one_provider() {
    assert_eq!(
        errs("      - cloudflare: { token: \"${CF}\" }\n", ""),
        [
            "web.letsencrypt.dns[0].domains is required: the domains this provider manages, like domains: [example.com]"
        ]
    );
    assert_eq!(
        errs(&entry("[example.com]", "wait_secs: 30"), ""),
        ["web.letsencrypt.dns[0]: name exactly one of cloudflare, rfc2136, acme_dns or command"]
    );
    let dir = tempfile::tempdir().unwrap();
    let two = entry("[example.com]", CF) + "        command: hook\n";
    assert_eq!(
        errors(&load_with(&hook_in(dir.path()), &two, "")),
        ["web.letsencrypt.dns[0]: name exactly one of cloudflare, rfc2136, acme_dns or command"]
    );
}

#[test]
fn domains_hold_no_wildcards_and_appear_in_one_entry_only() {
    assert_eq!(
        errs(&entry("[\"*.example.com\"]", CF), ""),
        [
            "web.letsencrypt.dns[0].domains: write the domain example.com; it already covers *.example.com and every name under it"
        ]
    );
    let two = entry("[example.com]", CF) + &entry("[example.net, Example.com]", CF);
    assert_eq!(
        errs(&two, ""),
        [
            "domain \"example.com\" is listed in web.letsencrypt.dns[0] and [1]; list it in one entry only"
        ]
    );
}

#[test]
fn wait_resolvers_rfc2136_and_the_command_are_checked() {
    assert_eq!(
        errs(
            &entry("[example.com]", &format!("{CF}\n        wait_secs: 5")),
            ""
        ),
        ["\"wait_secs\" in \"web.letsencrypt.dns[0]\" must be between 10 and 3600, found 5"]
    );
    assert_eq!(
        errs(
            &entry(
                "[example.com]",
                &format!("{CF}\n        resolvers: [one.one]")
            ),
            ""
        ),
        ["web.letsencrypt.dns[0].resolvers: \"one.one\" is not an IP address or ip:port"]
    );
    assert_eq!(
        errs(
            &entry(
                "[example.com]",
                "rfc2136: { server: ns1, key_name: k, secret: \"${TSIG}\" }"
            ),
            ""
        ),
        ["web.letsencrypt.dns[0].rfc2136: server \"ns1\" needs a port, like ns1:53"]
    );
    assert_eq!(
        errs(
            &entry(
                "[example.com]",
                "rfc2136: { server: \"::1:53\", key_name: k, secret: \"${TSIG}\" }"
            ),
            ""
        ),
        [
            "web.letsencrypt.dns[0].rfc2136.server: \"::1:53\" looks like an IPv6 address; write it in brackets, like [::1]:22"
        ]
    );
    assert!(
        errs(
            &entry(
                "[example.com]",
                "rfc2136: { server: \"[::1]:53\", key_name: k, secret: \"${TSIG}\" }"
            ),
            ""
        )
        .is_empty()
    );
    assert_eq!(
        errs(
            &entry(
                "[example.com]",
                "rfc2136: { server: \"ns1:53\", key_name: k, secret: \"${NOTB64}\" }"
            ),
            ""
        ),
        ["web.letsencrypt.dns[0].rfc2136.secret is not base64"]
    );
    assert!(warns(&entry("[example.com]", "rfc2136: { server: \"ns1:53\", key_name: k, secret: \"${SHORT}\" }"), "")
        .contains(&"web.letsencrypt.dns[0].rfc2136.secret is 6 bytes; RFC 8945 asks for at least 32 for hmac-sha256".to_string()));
    assert_eq!(
        errs(&entry("[example.com]", "command: /no/such/burrow-dns"), ""),
        ["web.letsencrypt.dns[0].command: /no/such/burrow-dns does not exist or is not executable"]
    );
}

#[test]
fn a_command_is_a_path_not_a_file_secret() {
    assert_eq!(
        errs(
            &entry(
                "[example.com]",
                "command: \"file:/usr/local/bin/burrow-dns\""
            ),
            ""
        ),
        [
            "web.letsencrypt.dns[0].command: file:/usr/local/bin/burrow-dns is the path of a program, not a secret, so file: does not belong there"
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_command_others_can_change_is_a_warning() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let hook = dir.path().join("hook.sh");
    std::fs::write(&hook, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o777)).unwrap();
    let o = load_with(dir.path(), &entry("[example.com]", "command: hook.sh"), "");
    assert!(errors(&o).is_empty(), "{:#?}", errors(&o));
    assert!(o.report.diagnostics.iter().any(|d| d.message
        == "web.letsencrypt.dns[0].command: hook.sh can be changed by users other than its owner"));
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = load_with(dir.path(), &entry("[example.com]", "command: hook.sh"), "");
    assert!(
        !o.report.diagnostics.iter().any(|d| d.code == "dns-command"),
        "{:#?}",
        o.report.diagnostics
    );
}

#[test]
fn a_shared_wildcard_sits_on_the_entry_that_manages_its_name() {
    let one = |w: &str| {
        entry(
            "[example.com]",
            &format!("{CF}\n        wildcards: [\"{w}\"]"),
        )
    };
    assert_eq!(
        errs(&one("apps.example.com"), ""),
        [
            "web.letsencrypt.dns[0].wildcards: \"apps.example.com\" is not a wildcard; write it as \"*.apps.example.com\""
        ]
    );
    assert_eq!(
        errs(&one("*.apps.example.org"), ""),
        [
            "web.letsencrypt.dns[0].wildcards: \"*.apps.example.org\" is not under this entry's domains (example.com)"
        ]
    );
    let wrong = one("*.apps.example.com") + &entry("[apps.example.com]", CF);
    assert_eq!(
        errs(&wrong, ""),
        [
            "web.letsencrypt.dns[0].wildcards: \"*.apps.example.com\" belongs to apps.example.com, which dns[1] manages; list it in dns[1].wildcards"
        ]
    );
    assert!(
        warns(&one("*.example.com"), "")
            .contains(&"wildcard *.example.com is used by nothing yet".to_string())
    );
    assert!(
        !warns(&one("*.example.com"), &site("wiki", "wiki.example.com"))
            .iter()
            .any(|w| w.contains("used by nothing"))
    );
}

#[test]
fn a_dns_check_takes_the_entry_of_the_names_domain_or_says_which_domain_to_add() {
    let dns = entry("[example.com]", CF);
    assert_eq!(
        errs(
            &dns,
            &site("apps", "[\"*.apps.example.com\", apps.example.com]")
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        errs(
            &dns,
            &site("wiki", "{ hosts: [wiki.example.com], challenge: dns }")
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        errs(
            &dns,
            &site("erp", "{ hosts: [erp.corp.example.org], challenge: dns }")
        ),
        [
            "services.erp.web.hosts: \"erp.corp.example.org\" needs a DNS check (challenge: dns), but no web.letsencrypt.dns entry lists corp.example.org (or a domain above it)"
        ]
    );
    assert_eq!(
        errs(&dns, &site("apps", "\"*.apps.example.org\"")),
        [
            "services.apps.web.hosts: \"*.apps.example.org\" needs a DNS check (a wildcard), but no web.letsencrypt.dns entry lists apps.example.org (or a domain above it)"
        ]
    );
}

#[test]
fn three_sites_under_one_dns_domain_get_the_wildcard_hint_once() {
    let dns = entry("[example.com]", CF);
    let three = site("wiki", "wiki.example.com")
        + &site("blog", "blog.example.com")
        + &site("auth", "auth.example.com");
    let found: Vec<String> = warns(&dns, &three)
        .into_iter()
        .filter(|w| w.contains("one wildcard"))
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found,
        [
            "auth, blog and wiki.example.com each get their own certificate; one wildcard would cover them: add \"*.example.com\" to wildcards in the letsencrypt.dns entry for example.com"
        ]
    );
    let covered = entry(
        "[example.com]",
        &format!("{CF}\n        wildcards: [\"*.example.com\"]"),
    );
    assert!(
        !warns(&covered, &three)
            .iter()
            .any(|w| w.contains("one wildcard"))
    );
    let two = site("wiki", "wiki.example.com") + &site("blog", "blog.example.com");
    assert!(!warns(&dns, &two).iter().any(|w| w.contains("one wildcard")));
}

#[test]
fn the_tunnel_name_takes_its_dns_check_through_the_entries_too() {
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "clients:\n  home: {{ secret: \"{SECRET}\" }}\nweb:\n  https: 8443\n  letsencrypt:\n    email: ops@example.com\nwebsocket:\n  listen: 8443\n  host: tunnel.example.com\n  challenge: dns\n"
    );
    let o = load_str_in(
        dir.path(),
        "server.yaml",
        &text,
        Format::Yaml,
        Some(Role::Server),
        &|_: &str| None,
    );
    assert_eq!(
        errors(&o),
        [
            "websocket.challenge: dns needs web.letsencrypt.dns: the provider that sets the DNS record"
        ]
    );
}

#[test]
fn a_name_under_a_shared_wildcard_needs_no_check_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "clients:\n  home: {{ secret: \"{SECRET}\" }}\nlisten: 7835\nweb:\n  https: 8443\n  letsencrypt:\n    email: ops@example.com\n    dns:\n      - domains: [example.com]\n        cloudflare: {{ token: \"${{CF}}\" }}\n        wildcards: [\"*.example.com\"]\nservices:\n{}{}",
        site("wiki", "wiki.example.com"),
        site("root", "example.com")
    );
    let env = |n: &str| (n == "CF").then(|| "token".to_string());
    let o = load_str_in(
        dir.path(),
        "server.yaml",
        &text,
        Format::Yaml,
        Some(Role::Server),
        &env,
    );
    assert_eq!(
        errors(&o),
        Vec::<String>::new(),
        "8443 and no port 80, but the wildcard covers both"
    );
    let bare = text.replace("        wildcards: [\"*.example.com\"]\n", "");
    let o = load_str_in(
        dir.path(),
        "server.yaml",
        &bare,
        Format::Yaml,
        Some(Role::Server),
        &env,
    );
    assert_eq!(errors(&o).len(), 2, "{:#?}", errors(&o));
}

#[cfg(feature = "web")]
#[test]
fn the_tsig_algorithm_is_one_of_three_and_the_secret_should_be_as_long_as_its_mac() {
    let rfc = |alg: &str| {
        entry(
            "[example.com]",
            &format!(
                "rfc2136: {{ server: \"ns1:53\", key_name: k, secret: \"${{TSIG}}\", algorithm: \"{alg}\" }}"
            ),
        )
    };
    assert_eq!(errs(&rfc("HMAC-SHA384."), ""), Vec::<String>::new());
    assert!(
        warns(&rfc("hmac-sha256"), "")
            .iter()
            .all(|w| !w.contains("bytes"))
    );
    assert!(
        warns(&rfc("hmac-sha512"), "").contains(
            &"web.letsencrypt.dns[0].rfc2136.secret is 32 bytes; RFC 8945 asks for at least 64 for hmac-sha512".to_string()
        )
    );
    assert_eq!(
        errs(&rfc("hmac-md5"), ""),
        [
            "web.letsencrypt.dns[0].rfc2136.algorithm: \"hmac-md5\" is not hmac-sha256, hmac-sha384 or hmac-sha512"
        ]
    );
}

#[cfg(feature = "web")]
#[test]
fn an_acme_dns_api_is_https_or_plain_http_to_a_private_address() {
    let api = |url: &str| {
        entry(
            "[example.com]",
            &format!(
                "acme_dns: {{ api: \"{url}\", username: \"${{CF}}\", password: \"${{CF}}\", subdomain: abc }}"
            ),
        )
    };
    for ok in [
        "https://auth.acme-dns.io",
        "http://192.168.1.5:8080",
        "http://127.0.0.1",
        "http://[fd00::5]/",
        "http://acme.lan",
    ] {
        assert_eq!(errs(&api(ok), ""), Vec::<String>::new(), "{ok}");
    }
    assert_eq!(
        errs(&api("http://203.0.113.5"), ""),
        [
            "web.letsencrypt.dns[0].acme_dns.api: 203.0.113.5 is a public address; plain http:// is allowed only to loopback or private addresses, so use https://"
        ]
    );
    assert_eq!(
        errs(&api("ftp://auth.example.org"), ""),
        [
            "web.letsencrypt.dns[0].acme_dns.api: \"ftp://auth.example.org\" is not an https:// address, like https://auth.example.org"
        ]
    );
}
