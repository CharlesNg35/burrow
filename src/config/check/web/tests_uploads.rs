use std::path::Path;

use super::tests::{SECRET, errors, settings};
use crate::config::load::load_str_at;
use crate::config::settings::{TargetTls, WebTls};
use crate::config::{Format, Outcome, Role};
use burrow_web::tls::uploads::{Kind, id_of};

const TOP: &str = "listen: 7835\nweb: { https: 443, http: 80 }\nservices:\n";

fn load(state: Option<&Path>, services: &str) -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    let text = format!("clients:\n  home: {{ secret: \"{SECRET}\" }}\n{TOP}{services}");
    load_str_at(
        (dir.path(), state),
        "server.yaml",
        &text,
        Format::Yaml,
        Some(Role::Server),
        &|_: &str| None,
    )
}

fn site(web: &str) -> String {
    format!("  nas:\n    client: home\n    target: 5000\n    web: {web}\n")
}

fn store(state: &Path, kind: Kind, pem: &str) -> String {
    let id = id_of(kind, pem.as_bytes());
    let dir = state.join("files");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{id}.pem")), pem).unwrap();
    id
}

fn pair_for(name: &str) -> String {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec![name.to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    format!("{}{}", cert.pem(), key.serialize_pem())
}

fn ca_pem() -> String {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.self_signed(&key).unwrap().pem()
}

#[test]
fn an_uploaded_pair_serves_the_site_from_the_state_folder() {
    let state = tempfile::tempdir().unwrap();
    let id = store(state.path(), Kind::Pair, &pair_for("nas.example.com"));
    let s = settings(load(
        Some(state.path()),
        &site(&format!(
            "{{ hosts: [nas.example.com], tls: {{ upload: {id} }} }}"
        )),
    ));
    let web = s.web.unwrap();
    let file = state.path().join("files").join(format!("{id}.pem"));
    assert_eq!(
        web.sites["nas"].tls,
        WebTls::Files {
            cert: file.clone(),
            key: file
        }
    );
    assert_eq!(web.state_dir.as_deref(), Some(state.path()));
}

#[test]
fn an_uploaded_pair_must_cover_the_sites_names() {
    let state = tempfile::tempdir().unwrap();
    let id = store(state.path(), Kind::Pair, &pair_for("other.example.com"));
    let e = errors(&load(
        Some(state.path()),
        &site(&format!(
            "{{ hosts: [nas.example.com], tls: {{ upload: {id} }} }}"
        )),
    ));
    assert!(
        e.iter()
            .any(|m| m.contains("does not cover \"nas.example.com\"")),
        "{e:#?}"
    );
}

#[test]
fn a_file_comes_from_a_path_or_an_upload_never_both_or_neither() {
    let state = tempfile::tempdir().unwrap();
    let id = store(state.path(), Kind::Pair, &pair_for("nas.example.com"));
    let errs = |web: &str| errors(&load(Some(state.path()), &site(web)));
    assert_eq!(
        errs(&format!(
            "{{ hosts: [nas.example.com], tls: {{ cert: a.pem, key: a.key, upload: {id} }} }}"
        )),
        ["services.nas.web.tls: set cert and key, or upload a certificate, not both"]
    );
    assert_eq!(
        errs("{ hosts: [nas.example.com], tls: { cert: a.pem } }"),
        ["services.nas.web.tls: cert and key go together"]
    );
    assert_eq!(
        errs("{ hosts: [nas.example.com], tls: {} }"),
        ["services.nas.web.tls: needs cert and key, or an uploaded certificate"]
    );
    assert_eq!(
        errs("{ hosts: [nas.lan], tls: internal, target_tls: {} }"),
        ["services.nas.web.target_tls: needs a CA: a file, or an upload"]
    );
}

#[test]
fn an_upload_id_names_its_kind_and_a_file_burrow_kept() {
    let state = tempfile::tempdir().unwrap();
    let ca = store(state.path(), Kind::Ca, &ca_pem());
    let errs = |web: &str| errors(&load(Some(state.path()), &site(web)));
    assert_eq!(
        errs(&format!(
            "{{ hosts: [nas.example.com], tls: {{ upload: {ca} }} }}"
        )),
        [format!(
            "services.nas.web.tls: \"{ca}\" is not an upload of a certificate and its key"
        )]
    );
    assert_eq!(
        errs("{ hosts: [nas.example.com], tls: { upload: ../../etc/passwd } }"),
        [
            "services.nas.web.tls: \"../../etc/passwd\" is not an upload of a certificate and its key"
        ]
    );
    let gone = "pair-0123456789abcdef";
    assert_eq!(
        errs(&format!(
            "{{ hosts: [nas.example.com], tls: {{ upload: {gone} }} }}"
        )),
        [format!(
            "services.nas.web.tls: the upload {gone} is not in {}",
            state.path().join("files").display()
        )]
    );
    assert_eq!(
        errors(&load(
            None,
            &site(&format!(
                "{{ hosts: [nas.lan], tls: internal, target_tls: {{ ca_upload: {ca} }} }}"
            ))
        )),
        [
            "services.nas.web.target_tls: uploads live in the server's state folder, which is not known here"
        ]
    );
}

#[test]
fn an_uploaded_ca_checks_the_app() {
    let state = tempfile::tempdir().unwrap();
    let ca = store(state.path(), Kind::Ca, &ca_pem());
    let s = settings(load(
        Some(state.path()),
        &site(&format!(
            "{{ hosts: [nas.lan], tls: internal, target_tls: {{ ca_upload: {ca} }} }}"
        )),
    ));
    assert_eq!(
        s.web.unwrap().sites["nas"].options.target_tls,
        TargetTls::Ca(state.path().join("files").join(format!("{ca}.pem")))
    );
}

#[test]
fn tls_internal_serves_lan_names_and_addresses_only() {
    let errs = |hosts: &str| {
        errors(&load(
            None,
            &site(&format!("{{ hosts: {hosts}, tls: internal }}")),
        ))
    };
    for ok in [
        "[nas.lan]",
        "[x.home.arpa]",
        "[printer.local]",
        "[db.internal]",
        "[localhost, app.localhost]",
        "[\"*.nas.lan\"]",
        "[192.168.1.5, 10.0.0.2, \"fd00::5\", 100.64.1.1]",
    ] {
        assert_eq!(errs(ok), Vec::<String>::new(), "{ok}");
    }
    assert_eq!(
        errs("[nas.example.com]"),
        [
            "services.nas.web.hosts: nas.example.com is not a LAN name; tls: internal is only for names under localhost, local, internal, home.arpa, lan, so use tls: auto for public names, or name it like nas.lan"
        ]
    );
    assert_eq!(
        errs("[\"*.home.arpa\"]"),
        [
            "services.nas.web.hosts: *.home.arpa spans a whole suffix or a public domain; a local CA wildcard is one label under a LAN name, like *.nas.lan"
        ]
    );
    assert_eq!(
        errs("[8.8.8.8]"),
        [
            "services.nas.web.hosts: 8.8.8.8 is a public address; tls: internal serves only addresses of your own network, like 192.168.1.5 or fd00::5"
        ]
    );
    assert_eq!(
        errs("[nas]"),
        [
            "services.nas.web.hosts: \"nas\" is a single-label name; HTTPS finds sites by their full name, so use a full name like nas.lan or nas.home.arpa, or set tls: off to serve it as plain HTTP"
        ]
    );
    let s = settings(load(None, &site("{ hosts: [nas.lan], tls: internal }")));
    assert_eq!(s.web.unwrap().sites["nas"].tls, WebTls::Internal);
}

#[test]
fn tls_internal_takes_no_lets_encrypt_check() {
    assert_eq!(
        errors(&load(
            None,
            &site("{ hosts: [nas.lan], tls: internal, challenge: http }")
        )),
        [
            "services.nas.web.challenge: the local CA signs its certificates, so Let's Encrypt never checks it (tls: internal); remove challenge"
        ]
    );
}
