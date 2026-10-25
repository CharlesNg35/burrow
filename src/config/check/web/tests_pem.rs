use std::path::Path;

use super::tests::load_in;

fn pair(dir: &Path, stem: &str, names: &[&str]) {
    let key = rcgen::KeyPair::generate().unwrap();
    let names = names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    let cert = rcgen::CertificateParams::new(names)
        .unwrap()
        .self_signed(&key)
        .unwrap();
    std::fs::write(dir.join(format!("{stem}.pem")), cert.pem()).unwrap();
    std::fs::write(dir.join(format!("{stem}.key")), key.serialize_pem()).unwrap();
}

fn file_errors(dir: &Path, rest: &str) -> Vec<String> {
    load_in(dir, rest)
        .report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect()
}

const SHARED: &str = "listen: 7835\nweb:\n  https: 443\n  certificates:\n    - hosts: [\"*.example.com\", example.com]\n      cert: w.pem\n      key: w.key\nservices:\n  wiki: { client: home, target: 3000, web: wiki.example.com }\n";

const OWN: &str = "listen: 7835\nweb:\n  https: 443\nservices:\n  portal:\n    client: home\n    target: 3000\n    web:\n      hosts: [portal.example.com, www.portal.example.com]\n      tls: { cert: p.pem, key: p.key }\n";

#[test]
fn certificate_files_that_load_and_cover_their_names_pass() {
    let dir = tempfile::tempdir().unwrap();
    pair(dir.path(), "w", &["*.example.com", "example.com"]);
    pair(
        dir.path(),
        "p",
        &["portal.example.com", "www.portal.example.com"],
    );
    assert!(file_errors(dir.path(), SHARED).is_empty());
    assert!(file_errors(dir.path(), OWN).is_empty());
}

#[test]
fn a_missing_or_unreadable_file_is_refused_at_validate() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        file_errors(dir.path(), SHARED),
        [
            "certificate-file: web.certificates[0]: cannot read cert \"w.pem\": file not found",
            "certificate-file: web.certificates[0]: cannot read key \"w.key\": file not found",
        ]
    );
    pair(dir.path(), "p", &["portal.example.com"]);
    std::fs::remove_file(dir.path().join("p.key")).unwrap();
    assert_eq!(
        file_errors(dir.path(), OWN),
        ["certificate-file: services.portal.web.tls: cannot read key \"p.key\": file not found"]
    );
}

#[test]
fn a_file_that_is_not_pem_or_a_key_that_does_not_fit_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    pair(dir.path(), "w", &["*.example.com", "example.com"]);
    std::fs::write(dir.path().join("w.pem"), "not a certificate\n").unwrap();
    let e = file_errors(dir.path(), SHARED);
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(
        e[0].starts_with(
            "certificate-file: web.certificates[0]: w.pem contains no PEM certificate"
        ),
        "{e:?}"
    );
    pair(dir.path(), "w", &["*.example.com", "example.com"]);
    pair(dir.path(), "other", &["*.example.com"]);
    std::fs::copy(dir.path().join("other.key"), dir.path().join("w.key")).unwrap();
    let e = file_errors(dir.path(), SHARED);
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(
        e[0].starts_with("certificate-file: web.certificates[0]: w.key does not fit w.pem"),
        "{e:?}"
    );
}

#[test]
fn a_file_must_cover_each_name_it_is_used_for() {
    let dir = tempfile::tempdir().unwrap();
    pair(dir.path(), "w", &["*.example.com"]);
    pair(dir.path(), "p", &["portal.example.com"]);
    assert_eq!(
        file_errors(dir.path(), SHARED),
        [
            "certificate-cover: web.certificates[0]: w.pem does not cover \"example.com\" listed in its hosts"
        ]
    );
    assert_eq!(
        file_errors(dir.path(), OWN),
        [
            "certificate-cover: services.portal.web.tls: p.pem does not cover \"www.portal.example.com\"; add the name to the certificate, move the name to another service, or list it in web.certificates"
        ]
    );
}

#[test]
fn a_shared_file_that_covers_more_than_it_lists_gets_a_note() {
    let dir = tempfile::tempdir().unwrap();
    pair(
        dir.path(),
        "w",
        &[
            "*.example.com",
            "example.com",
            "a.example.com",
            "Other.example.net",
        ],
    );
    let notes: Vec<String> = load_in(dir.path(), SHARED)
        .report
        .diagnostics
        .iter()
        .filter(|d| d.code == "certificate-extra")
        .map(|d| format!("{}{}", if d.is_error() { "error: " } else { "" }, d.message))
        .collect();
    assert_eq!(
        notes,
        [
            "web.certificates[0]: w.pem also covers \"other.example.net\", which its hosts does not list; it is used only for the listed names"
        ]
    );
    pair(dir.path(), "w", &["*.example.com", "example.com"]);
    assert!(
        load_in(dir.path(), SHARED)
            .report
            .diagnostics
            .iter()
            .all(|d| d.code != "certificate-extra")
    );
}
