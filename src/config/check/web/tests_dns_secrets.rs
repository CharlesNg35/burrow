use crate::config::load::load_str_in;
use crate::config::{Format, Outcome, Role};

use super::tests::SECRET;

fn load(dir: &std::path::Path, provider: &str) -> (String, Outcome) {
    let text = format!(
        "clients:\n  home: {{ secret: \"{SECRET}\" }}\nweb:\n  https: 443\n  letsencrypt:\n    email: ops@example.com\n    dns:\n      - domains: [example.com]\n        {provider}\n"
    );
    let env = |name: &str| match name {
        "CF_TOKEN" => Some("cf-from-env".to_string()),
        "EMPTY" => Some(String::new()),
        _ => None,
    };
    let o = load_str_in(
        dir,
        "server.yaml",
        &text,
        Format::Yaml,
        Some(Role::Server),
        &env,
    );
    (text, o)
}

fn secret_errors(o: &Outcome) -> Vec<String> {
    o.report
        .diagnostics
        .iter()
        .filter(|d| d.code == "dns-secret")
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn a_dns_secret_comes_from_the_environment_or_a_readable_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tsig.secret"), "c2VjcmV0\n").unwrap();
    for ok in [
        "cloudflare: { token: \"${CF_TOKEN}\" }",
        "cloudflare: { token: \"file:tsig.secret\" }",
        "rfc2136: { server: \"10.0.0.53:53\", key_name: acme, secret: \"file:tsig.secret\" }",
        "acme_dns: { api: \"https://auth.example.org\", username: user1, password: \"${CF_TOKEN}\", subdomain: abc }",
    ] {
        let (_, o) = load(dir.path(), ok);
        assert_eq!(secret_errors(&o), Vec::<String>::new(), "{ok}");
    }
}

#[test]
fn a_dns_secret_written_in_the_file_unreadable_or_empty_is_an_error_with_its_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("empty.token"), "\n").unwrap();
    let cases = [
        (
            "cloudflare: { token: cf-inline-123 }",
            "web.letsencrypt.dns[0].cloudflare.token is written in the file; use ${CF_TOKEN} or file: so it stays out of the config",
        ),
        (
            "rfc2136: { server: \"10.0.0.53:53\", key_name: acme, secret: c2VjcmV0 }",
            "web.letsencrypt.dns[0].rfc2136.secret is written in the file; use ${TSIG_SECRET} or file: so it stays out of the config",
        ),
        (
            "acme_dns: { api: \"https://auth.example.org\", username: u, password: pw, subdomain: abc }",
            "web.letsencrypt.dns[0].acme_dns.password is written in the file; use ${ACMEDNS_KEY} or file: so it stays out of the config",
        ),
        (
            "cloudflare: { token: \"file:empty.token\" }",
            "web.letsencrypt.dns[0].cloudflare.token: empty.token is empty",
        ),
        (
            "cloudflare: { token: \"${EMPTY}\" }",
            "web.letsencrypt.dns[0].cloudflare.token is empty",
        ),
    ];
    for (provider, want) in cases {
        let (_, o) = load(dir.path(), provider);
        assert_eq!(secret_errors(&o), [want.to_string()], "{provider}");
    }
    let (_, o) = load(dir.path(), "cloudflare: { token: \"file:missing.token\" }");
    let errs = secret_errors(&o);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(
        errs[0].starts_with("web.letsencrypt.dns[0].cloudflare.token: cannot read missing.token: "),
        "{errs:?}"
    );
}

#[test]
fn a_dns_secret_written_in_the_file_is_masked_in_the_report() {
    let dir = tempfile::tempdir().unwrap();
    let (text, o) = load(dir.path(), "cloudflare: { token: cf-inline-123 }");
    let masked: Vec<&str> = o
        .report
        .masked
        .iter()
        .map(|s| &text[s.start.offset..s.end.offset])
        .collect();
    assert!(masked.contains(&"cf-inline-123"), "{masked:?}");
    assert!(
        !masked.iter().any(|m| m.contains("example.com")),
        "{masked:?}"
    );
}

#[test]
fn a_misspelled_dns_provider_still_has_its_secret_masked() {
    let dir = tempfile::tempdir().unwrap();
    let (text, o) = load(dir.path(), "cloudflar: { token: hunter2hunter2, x: 1 }");
    let masked: Vec<&str> = o
        .report
        .masked
        .iter()
        .map(|s| &text[s.start.offset..s.end.offset])
        .collect();
    assert!(masked.contains(&"hunter2hunter2"), "{masked:?}");
    let shown = o
        .report
        .render_text(crate::base::style::Style { color: false });
    assert!(!shown.contains("hunter2hunter2"), "{shown}");
}
