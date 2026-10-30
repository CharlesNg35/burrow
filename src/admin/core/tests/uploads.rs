use burrow_admin_proto::msg::{AdminLevel, ErrorCode, Upload, Uploaded, req};
use burrow_syntax::upload::Kind;
use serde_json::json;

use super::{core_for, session, world};
use crate::admin::core::dispatch;

fn pair(name: &str) -> String {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec![name.to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    format!("{}{}", cert.pem(), key.serialize_pem())
}

fn body(kind: &str, pem: &str, hosts: &[&str]) -> serde_json::Value {
    serde_json::to_value(Upload {
        kind: kind.into(),
        pem: pem.into(),
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
        ca_of: None,
        ca_upload: None,
    })
    .unwrap()
}

#[test]
fn a_write_admin_uploads_a_pair_and_gets_its_id_and_facts_never_the_key() {
    let w = world();
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let pem = pair("nas.example.com");
    let b = body(Kind::Pair.prefix(), &pem, &["nas.example.com"]);
    let reply = dispatch(&core, &s, req::UPLOAD, &b).unwrap();
    let up: Uploaded = serde_json::from_value(reply.clone()).unwrap();
    assert!(up.id.starts_with("pair-"), "{}", up.id);
    assert_eq!(up.names, ["nas.example.com"]);
    assert!(!reply.to_string().contains("PRIVATE KEY"));
    let file = core
        .store
        .state_dir()
        .join("files")
        .join(format!("{}.pem", up.id));
    assert_eq!(std::fs::read_to_string(file).unwrap(), pem);
    let again: Uploaded =
        serde_json::from_value(dispatch(&core, &s, req::UPLOAD, &b).unwrap()).unwrap();
    assert_eq!(again.id, up.id, "the same file twice is one upload");
}

#[test]
fn an_upload_is_refused_to_read_admins_and_when_it_does_not_fit() {
    let w = world();
    let core = core_for(&w.text);
    let pem = pair("nas.example.com");
    let read = session(&w, AdminLevel::Read);
    let e = dispatch(
        &core,
        &read,
        req::UPLOAD,
        &body(Kind::Pair.prefix(), &pem, &[]),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden);
    let s = session(&w, AdminLevel::Write);
    let e = dispatch(&core, &s, req::UPLOAD, &body("nonsense", &pem, &[])).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    let wrong = body(Kind::Pair.prefix(), &pem, &["other.example.com"]);
    let e = dispatch(&core, &s, req::UPLOAD, &wrong).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    let e = dispatch(&core, &s, req::UPLOAD, &body(Kind::Crl.prefix(), "x", &[])).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert!(!core.store.state_dir().join("files").exists());
}

#[test]
fn any_admin_can_fetch_the_local_cas_certificate_once_it_exists() {
    let w = world();
    let core = core_for(&w.text);
    let read = session(&w, AdminLevel::Read);
    let e = dispatch(&core, &read, req::LOCAL_CA, &json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    let root = crate::server::uploads::root_pem(&core.store.state_dir());
    std::fs::create_dir_all(root.parent().unwrap()).unwrap();
    std::fs::write(
        &root,
        "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n",
    )
    .unwrap();
    let got = dispatch(&core, &read, req::LOCAL_CA, &json!({})).unwrap();
    assert!(
        got["pem"]
            .as_str()
            .unwrap()
            .starts_with("-----BEGIN CERTIFICATE-----")
    );
    assert!(!got.to_string().contains("PRIVATE"));
}

#[test]
fn a_local_ca_that_cannot_be_read_says_why_instead_of_that_there_is_none() {
    let w = world();
    let core = core_for(&w.text);
    let read = session(&w, AdminLevel::Read);
    let root = crate::server::uploads::root_pem(&core.store.state_dir());
    std::fs::create_dir_all(&root).unwrap();
    let e = dispatch(&core, &read, req::LOCAL_CA, &json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Internal, "{}", e.message);
    assert!(e.message.starts_with("cannot read "), "{}", e.message);
}
