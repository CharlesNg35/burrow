use std::sync::Arc;

use tokio_rustls::rustls::RootCertStore;
use tokio_rustls::rustls::client::WebPkiServerVerifier;
use tokio_rustls::rustls::client::danger::ServerCertVerifier;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use x509_cert::certificate::{CertificateInner, Raw};
use x509_cert::der::{Decode, Encode};
use x509_cert::ext::pkix::{BasicConstraints, ExtendedKeyUsage, NameConstraints};

use super::*;

const NOW: u64 = 1_800_000_000;

fn writer() -> Writer {
    default_writer()
}

fn cert(der: &[u8]) -> CertificateInner<Raw> {
    CertificateInner::<Raw>::from_der(der).unwrap()
}

fn verify(
    root: &CertificateDer<'static>,
    chain: &[CertificateDer<'static>],
    name: &str,
    at: u64,
) -> Result<(), String> {
    let mut roots = RootCertStore::empty();
    roots.add(root.clone()).unwrap();
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let v = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
        .build()
        .unwrap();
    let at = UnixTime::since_unix_epoch(std::time::Duration::from_secs(at));
    v.verify_server_cert(
        &chain[0],
        &chain[1..],
        &ServerName::try_from(name.to_string()).unwrap(),
        &[],
        at,
    )
    .map(|_| ())
    .map_err(|e| format!("{e:?}"))
}

#[test]
fn the_ca_is_made_once_with_private_files_and_reopened_with_the_same_root() {
    let state = tempfile::tempdir().unwrap();
    let folder = dir(state.path());
    let w = writer();
    let made = Ca::create(&folder, NOW, &*w).unwrap();
    assert!(
        made.subject().starts_with("burrow local CA "),
        "{}",
        made.subject()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&folder), 0o700);
        for f in [
            "root.pem",
            "root.key",
            "intermediate.pem",
            "intermediate.key",
            "ca.json",
        ] {
            assert_eq!(mode(&folder.join(f)), 0o600, "{f}");
        }
    }
    assert!(Ca::exists(&folder));
    let again = Ca::open(&folder, NOW + 60, &*w).unwrap();
    assert_eq!(fingerprint(&again.root), fingerprint(&made.root));
    assert_eq!(again.intermediate.der, made.intermediate.der);
    assert_eq!(
        made.root_not_after - made.root_not_before,
        ROOT_SECS + SKEW_SECS
    );
}

#[test]
fn root_and_intermediate_carry_the_critical_name_constraints_and_their_path_lengths() {
    let state = tempfile::tempdir().unwrap();
    let ca = Ca::create(&dir(state.path()), NOW, &*writer()).unwrap();
    for (der, len) in [(ca.root.as_ref(), 1u8), (ca.intermediate.der.as_ref(), 0u8)] {
        let c = cert(der);
        let tbs = c.tbs_certificate();
        let (critical, nc) = tbs.get_extension::<NameConstraints>().unwrap().unwrap();
        assert!(critical);
        let permitted = nc.permitted_subtrees.unwrap();
        assert_eq!(
            permitted.len(),
            PERMITTED_NAMES.len() + PERMITTED_NETS.len()
        );
        assert!(nc.excluded_subtrees.is_none());
        let (_, bc) = tbs.get_extension::<BasicConstraints>().unwrap().unwrap();
        assert!(bc.ca);
        assert_eq!(bc.path_len_constraint, Some(len));
    }
    let i = cert(&ca.intermediate.der);
    assert_eq!(
        i.tbs_certificate().issuer().to_der().unwrap(),
        cert(&ca.root).tbs_certificate().subject().to_der().unwrap()
    );
    assert_eq!(
        ca.intermediate.not_after - ca.intermediate.not_before,
        INTERMEDIATE_SECS + SKEW_SECS
    );
}

#[test]
fn leaves_are_short_have_no_subject_and_verify_only_for_lan_names() {
    let state = tempfile::tempdir().unwrap();
    let ca = Ca::create(&dir(state.path()), NOW, &*writer()).unwrap();
    let names: Vec<String> = ["nas.lan", "x.home.arpa", "192.168.1.5", "fd00::1"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let (key, not_after) = ca.issue(&names, NOW).unwrap();
    assert_eq!(not_after, NOW + LEAF_SECS);
    assert_eq!(key.cert.len(), 2, "leaf and intermediate, never the root");
    let leaf = cert(&key.cert[0]);
    let tbs = leaf.tbs_certificate();
    assert_eq!(
        tbs.subject().to_der().unwrap(),
        vec![0x30, 0x00],
        "an empty subject"
    );
    assert_eq!(
        tbs.validity().not_before.to_unix_duration().as_secs(),
        NOW - SKEW_SECS
    );
    let (_, eku) = tbs.get_extension::<ExtendedKeyUsage>().unwrap().unwrap();
    assert_eq!(
        eku.0,
        vec![x509_cert::spki::ObjectIdentifier::new_unwrap(
            "1.3.6.1.5.5.7.3.1"
        )]
    );
    assert_eq!(
        tbs.issuer().to_der().unwrap(),
        cert(&ca.intermediate.der)
            .tbs_certificate()
            .subject()
            .to_der()
            .unwrap()
    );
    for n in &names {
        verify(&ca.root, &key.cert, n, NOW + 60).unwrap_or_else(|e| panic!("{n}: {e}"));
    }
    let (public, _) = ca.issue(&["example.com".to_string()], NOW).unwrap();
    let refused = verify(&ca.root, &public.cert, "example.com", NOW + 60).unwrap_err();
    assert!(refused.contains("NameConstraint"), "{refused}");
    let (ip, _) = ca.issue(&["8.8.8.8".to_string()], NOW).unwrap();
    assert!(verify(&ca.root, &ip.cert, "8.8.8.8", NOW + 60).is_err());
}

#[test]
fn a_reopened_ca_signs_with_the_same_names_and_renews_its_intermediate_from_the_root() {
    let state = tempfile::tempdir().unwrap();
    let folder = dir(state.path());
    let w = writer();
    let first = Ca::create(&folder, NOW, &*w).unwrap();
    let reopened = Ca::open(&folder, NOW + 3600, &*w).unwrap();
    let (key, _) = reopened
        .issue(&["nas.lan".to_string()], NOW + 3600)
        .unwrap();
    assert_eq!(
        cert(&key.cert[0])
            .tbs_certificate()
            .issuer()
            .to_der()
            .unwrap(),
        cert(&first.intermediate.der)
            .tbs_certificate()
            .subject()
            .to_der()
            .unwrap()
    );
    verify(&first.root, &key.cert, "nas.lan", NOW + 3660).unwrap();
    std::fs::remove_file(folder.join("intermediate.pem")).unwrap();
    let resigned = Ca::open(&folder, NOW + 7200, &*w).unwrap();
    assert_ne!(resigned.intermediate.der, first.intermediate.der);
    assert_eq!(fingerprint(&resigned.root), fingerprint(&first.root));
    let (key, _) = resigned
        .issue(&["nas.lan".to_string()], NOW + 7200)
        .unwrap();
    verify(&first.root, &key.cert, "nas.lan", NOW + 7260).unwrap();
    let mut kept = Ca::open(&folder, NOW + 7200, &*w).unwrap();
    std::fs::remove_file(folder.join("root.key")).unwrap();
    let late = resigned.intermediate.not_after - INTERMEDIATE_SECS / 10;
    let err = kept.refresh(late, &*w).err().unwrap();
    assert!(err.contains("restore") && err.contains("root.key"), "{err}");
    assert!(Ca::exists(&folder), "nothing is made again");
}

#[test]
fn renewal_points_follow_caddys_ratios() {
    assert!(!due(0, 100, 79));
    assert!(due(0, 100, 80));
    assert!(!leaf_due(0, 300, 199));
    assert!(leaf_due(0, 300, 200));
}

#[test]
fn the_manager_issues_per_group_and_reports_the_ca_with_its_root_warning() {
    let state = tempfile::tempdir().unwrap();
    let local = Internal::default();
    let w = writer();
    local.plan(
        Some(state.path().to_path_buf()),
        BTreeMap::from([("nas".to_string(), vec!["nas.lan".to_string()])]),
    );
    local.step(NOW, &*w);
    assert!(local.error().is_none(), "{:?}", local.error());
    let leaf = local.leaf("nas").unwrap();
    assert_eq!(leaf.names, vec!["nas.lan"]);
    let status = local.status(NOW).unwrap();
    assert_eq!(status.root_path, root_pem_path(state.path()));
    assert_eq!(status.sha256.len(), 95);
    assert!(status.warning.is_none());
    let late = local.status(status.not_after - 30 * 86_400).unwrap();
    assert_eq!(
        late.warning.as_deref(),
        Some("the local CA expires in 30 days; burrow does not renew it on its own")
    );
    local.step(NOW + 60, &*w);
    assert_eq!(
        local.leaf("nas").unwrap().key.cert[0],
        leaf.key.cert[0],
        "not due yet"
    );
    local.step(NOW + 9 * 3600, &*w);
    assert_ne!(
        local.leaf("nas").unwrap().key.cert[0],
        leaf.key.cert[0],
        "renewed after 8 h"
    );
    local.plan(Some(state.path().to_path_buf()), BTreeMap::new());
    local.step(NOW + 9 * 3600, &*w);
    assert!(local.leaf("nas").is_none());
    let none = Internal::default();
    none.plan(
        None,
        BTreeMap::from([("x".to_string(), vec!["x.lan".to_string()])]),
    );
    none.step(NOW, &*w);
    assert_eq!(
        none.error().as_deref(),
        Some("burrow has no state folder for its local CA")
    );
}
