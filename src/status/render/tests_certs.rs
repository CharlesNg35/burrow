use super::certs::render_certs;
use super::tests::sample;
use super::*;
use crate::metrics::{AcmeStatus, CertSourceStatus, CertStateStatus, CertStatus};
use burrow_admin_proto::status::CertCheck;

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 86_400;

fn cert(name: &str, source: CertSourceStatus, state: CertStateStatus) -> CertStatus {
    CertStatus {
        name: name.into(),
        source,
        check: None,
        state,
        expires: None,
        next_renewal: None,
        last_error: None,
        code: None,
        explanation: None,
        failures: 0,
    }
}

fn shown(s: &Status) -> String {
    let mut out = String::new();
    render_certs(&mut out, s, Paint::PLAIN, NOW);
    out
}

#[test]
fn each_certificate_shows_its_source_state_expiry_and_next_renewal() {
    let mut s = sample();
    let le = CertSourceStatus::LetsEncrypt {
        directory: "https://acme-v02.api.letsencrypt.org/directory".into(),
    };
    s.certificates = vec![
        CertStatus {
            check: Some(CertCheck::Tls),
            expires: Some(NOW + 60 * DAY),
            next_renewal: Some(NOW + 30 * DAY),
            ..cert("example.com", le.clone(), CertStateStatus::Ok)
        },
        CertStatus {
            check: Some(CertCheck::Http),
            last_error: Some("urn:ietf:params:acme:error:dns: no such name".into()),
            code: None,
            explanation: Some("https://letsencrypt.org/docs/x".into()),
            failures: 3,
            next_renewal: Some(NOW + 600),
            ..cert(
                "wiki.example.com",
                le,
                CertStateStatus::Failing {
                    reason: "the DNS name does not exist".into(),
                },
            )
        },
        CertStatus {
            expires: Some(NOW - 2 * DAY),
            ..cert(
                "portal.example.com",
                CertSourceStatus::OwnFile {
                    path: "/etc/burrow/portal.pem".into(),
                },
                CertStateStatus::Ok,
            )
        },
    ];
    s.acme = Some(AcmeStatus {
        directory: "https://acme-v02.api.letsencrypt.org/directory".into(),
        account: Some("https://acme-v02.api.letsencrypt.org/acme/acct/42".into()),
        state_dir: "/var/lib/burrow/server".into(),
        queue: Vec::new(),
    });
    let text = shown(&s);
    assert!(text.contains("CERTIFICATES"), "{text}");
    let row = |name: &str| {
        text.lines()
            .find(|l| l.trim_start().starts_with(name))
            .unwrap()
            .to_string()
    };
    let ok = row("example.com");
    assert!(
        ok.contains("Let's Encrypt, TLS check") && ok.contains("ok"),
        "{ok}"
    );
    assert!(ok.contains("in 60 d") && ok.contains("in 30 d"), "{ok}");
    let failing = row("wiki.example.com");
    assert!(
        failing.contains("failing") && failing.contains("in 10 min"),
        "{failing}"
    );
    let own = row("portal.example.com");
    assert!(
        own.contains("own file /etc/burrow/portal.pem") && own.contains("expired 2 d"),
        "{own}"
    );
    assert!(
        text.contains("wiki.example.com: the DNS name does not exist (3 failures); see https://letsencrypt.org/docs/x"),
        "{text}"
    );
    assert!(
        text.contains("Let's Encrypt account https://acme-v02.api.letsencrypt.org/acme/acct/42, kept in /var/lib/burrow/server"),
        "{text}"
    );
}

#[test]
fn a_far_expiry_reads_as_a_date() {
    let mut s = sample();
    s.certificates = vec![CertStatus {
        expires: Some(4_102_444_800),
        ..cert(
            "portal.example.com",
            CertSourceStatus::OwnFile {
                path: "/etc/burrow/portal.pem".into(),
            },
            CertStateStatus::Ok,
        )
    }];
    assert!(shown(&s).contains("1 Jan 2100"), "{}", shown(&s));
}

#[test]
fn a_server_without_certificates_of_its_own_prints_nothing_for_them() {
    let mut s = sample();
    assert_eq!(shown(&s), "");
    s.certificates = vec![cert(
        "nas.example.com",
        CertSourceStatus::Passthrough,
        CertStateStatus::Ok,
    )];
    assert_eq!(shown(&s), "", "the app's own certificate is not burrow's");
}

#[test]
fn a_plain_site_reads_unencrypted_in_the_websites_table() {
    use burrow_admin_proto::web::{HostLine, PLAIN, WebSummary};
    let mut s = sample();
    s.web = Some(WebSummary {
        http: "[::]:80".into(),
        hosts: vec![HostLine {
            name: "wiki.lan.example.com".into(),
            kind: PLAIN.into(),
            service: "wiki".into(),
            ..HostLine::default()
        }],
        ..WebSummary::default()
    });
    let text = render(&s, Paint::PLAIN, None);
    assert!(text.contains("plain HTTP, unencrypted"), "{text}");
}
