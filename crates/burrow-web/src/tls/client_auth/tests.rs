use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rcgen::{
    BasicConstraints, CertificateParams, CertificateRevocationListParams, DistinguishedName,
    DnType, IsCa, Issuer, KeyIdMethod, KeyPair, KeyUsagePurpose, RevokedCertParams, SerialNumber,
};
use time::OffsetDateTime;
use tokio_rustls::rustls::pki_types::CertificateRevocationListDer;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use x509_cert::der::Encode;

use super::*;

struct TestCa {
    params: CertificateParams,
    key: KeyPair,
    pem: String,
}

fn ca(name: &str, parent: Option<&TestCa>) -> TestCa {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, name);
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let cert = match parent {
        Some(p) => params
            .signed_by(&key, &Issuer::from_params(&p.params, &p.key))
            .unwrap(),
        None => params.self_signed(&key).unwrap(),
    };
    TestCa {
        pem: cert.pem(),
        params,
        key,
    }
}

fn crl(by: &TestCa, revoked: &[u64], next: OffsetDateTime) -> Vec<u8> {
    let params = CertificateRevocationListParams {
        this_update: next - time::Duration::days(30),
        next_update: next,
        crl_number: SerialNumber::from(1u64),
        issuing_distribution_point: None,
        revoked_certs: revoked
            .iter()
            .map(|s| RevokedCertParams {
                serial_number: SerialNumber::from(*s),
                revocation_time: next - time::Duration::days(31),
                reason_code: None,
                invalidity_date: None,
            })
            .collect(),
        key_identifier_method: KeyIdMethod::Sha256,
    };
    let c = params
        .signed_by(&Issuer::from_params(&by.params, &by.key))
        .unwrap();
    c.pem().unwrap().into_bytes()
}

fn soon() -> OffsetDateTime {
    OffsetDateTime::now_utc() + time::Duration::days(7)
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn check(ca: &[u8], crl: Option<&[u8]>) -> Result<Vec<CrlNote>, ClientAuthProblem> {
    let dir = tempfile::tempdir().unwrap();
    let ca = write(dir.path(), "ca.pem", ca);
    let crl = crl.map(|c| write(dir.path(), "devices.crl", c));
    check_files(&ca, crl.as_deref(), SystemTime::now())
}

#[test]
fn a_ca_with_its_own_crl_passes_and_unreadable_files_name_the_path() {
    let root = ca("Devices CA", None);
    assert_eq!(check(root.pem.as_bytes(), None), Ok(vec![]));
    let list = crl(&root, &[7], soon());
    assert_eq!(check(root.pem.as_bytes(), Some(&list)), Ok(vec![]));
    let missing = check_files(Path::new("/nonexistent/ca.pem"), None, SystemTime::now());
    assert!(
        matches!(missing, Err(ClientAuthProblem::CaUnreadable(e)) if e.contains("/nonexistent/ca.pem"))
    );
    let dir = tempfile::tempdir().unwrap();
    let ca_file = write(dir.path(), "ca.pem", root.pem.as_bytes());
    let gone = check_files(
        &ca_file,
        Some(Path::new("/nonexistent/x.crl")),
        SystemTime::now(),
    );
    assert!(matches!(gone, Err(ClientAuthProblem::CrlUnreadable(_))));
}

#[test]
fn a_leaf_or_junk_is_not_a_ca_and_junk_is_not_a_crl() {
    let key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["a.example.com".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    assert_eq!(
        check(leaf.pem().as_bytes(), None),
        Err(ClientAuthProblem::NotCa)
    );
    assert_eq!(check(b"hello", None), Err(ClientAuthProblem::NotCa));
    let root = ca("Devices CA", None);
    assert_eq!(
        check(root.pem.as_bytes(), Some(b"hello")),
        Err(ClientAuthProblem::NotCrl)
    );
    let cert_as_crl = root.pem.replace("CERTIFICATE", "X509 CRL");
    assert_eq!(
        check(root.pem.as_bytes(), Some(cert_as_crl.as_bytes())),
        Err(ClientAuthProblem::NotCrl)
    );
}

#[test]
fn a_crl_from_another_ca_even_with_the_same_name_is_refused() {
    let root = ca("Devices CA", None);
    let other = ca("Other CA", None);
    let twin = ca("Devices CA", None);
    assert_eq!(
        check(root.pem.as_bytes(), Some(&crl(&other, &[], soon()))),
        Err(ClientAuthProblem::NotSignedByCa {
            issuer: "CN=Other CA".into()
        })
    );
    assert_eq!(
        check(root.pem.as_bytes(), Some(&crl(&twin, &[], soon()))),
        Err(ClientAuthProblem::NotSignedByCa {
            issuer: "CN=Devices CA".into()
        })
    );
    let e = ClientAuthProblem::NotSignedByCa {
        issuer: "CN=Other CA".into(),
    };
    assert_eq!(
        e.to_string(),
        "holds a CRL from \"CN=Other CA\", which is not signed by any CA in client_auth.ca"
    );
}

#[test]
fn a_two_tier_ca_takes_the_intermediates_crl_the_roots_or_both_in_one_file() {
    let root = ca("Root CA", None);
    let mid = ca("Intermediate CA", Some(&root));
    let both_cas = format!("{}{}", root.pem, mid.pem);
    let mid_crl = crl(&mid, &[3], soon());
    let root_crl = crl(&root, &[], soon());
    assert_eq!(check(both_cas.as_bytes(), Some(&mid_crl)), Ok(vec![]));
    assert_eq!(check(both_cas.as_bytes(), Some(&root_crl)), Ok(vec![]));
    let both = [mid_crl.clone(), root_crl].concat();
    assert_eq!(check(both_cas.as_bytes(), Some(&both)), Ok(vec![]));
    assert!(matches!(
        check(root.pem.as_bytes(), Some(&mid_crl)),
        Err(ClientAuthProblem::NotSignedByCa { .. })
    ));
}

#[test]
fn an_expired_crl_is_a_note_not_an_error() {
    let root = ca("Devices CA", None);
    let past = OffsetDateTime::now_utc() - time::Duration::days(2);
    let notes = check(root.pem.as_bytes(), Some(&crl(&root, &[1], past))).unwrap();
    let [
        CrlNote::Expired {
            issuer,
            next_update,
        },
    ] = notes.as_slice()
    else {
        panic!("{notes:?}");
    };
    assert_eq!(issuer, "CN=Devices CA");
    assert!(*next_update < SystemTime::now());
    assert!(
        notes[0]
            .to_string()
            .contains("it still applies, publish a new one")
    );
}

fn der_seq(content: &[u8]) -> Vec<u8> {
    let mut out = vec![0x30];
    let n = content.len();
    if n < 128 {
        out.push(u8::try_from(n).unwrap());
    } else {
        let bytes: Vec<u8> = n
            .to_be_bytes()
            .into_iter()
            .skip_while(|b| *b == 0)
            .collect();
        out.push(0x80 | u8::try_from(bytes.len()).unwrap());
        out.extend(bytes);
    }
    out.extend_from_slice(content);
    out
}

#[test]
fn a_version_one_crl_names_the_openssl_crlnumber_fix() {
    use x509_cert::certificate::Raw;
    use x509_cert::der::Decode;
    let root = ca("Devices CA", None);
    let pem = crl(&root, &[], soon());
    let der = CertificateRevocationListDer::from_pem_slice(&pem).unwrap();
    let parsed = x509_cert::crl::CertificateList::<Raw>::from_der(der.as_ref()).unwrap();
    let t = &parsed.tbs_cert_list;
    let mut tbs = Vec::new();
    tbs.extend(t.signature.to_der().unwrap());
    tbs.extend(t.issuer.to_der().unwrap());
    tbs.extend(t.this_update.to_der().unwrap());
    tbs.extend(t.next_update.unwrap().to_der().unwrap());
    let mut body = der_seq(&tbs);
    body.extend(parsed.signature_algorithm.to_der().unwrap());
    body.extend(parsed.signature.to_der().unwrap());
    let v1 = der_seq(&body);
    let e = check(root.pem.as_bytes(), Some(&v1)).unwrap_err();
    assert_eq!(e, ClientAuthProblem::CrlV1);
    assert!(e.to_string().contains("crlnumber"));
}

#[test]
fn a_bad_replacement_keeps_the_last_good_files_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let root = ca("Devices CA", None);
    let key = ClientAuth {
        ca: write(dir.path(), "ca.pem", root.pem.as_bytes()),
        crl: Some(write(dir.path(), "d.crl", &crl(&root, &[], soon()))),
    };
    let resolver: Arc<dyn ResolvesServerCert> = Arc::new(crate::tls::Fixed::new(
        crate::tls::files::tests::certified_for("x.example.com"),
    ));
    let v = Verifiers::new(resolver);
    v.refresh([&key]);
    let first = v.config(&key, false).unwrap();
    assert!(v.notes(SystemTime::now()).is_empty());
    v.refresh([&key]);
    assert!(Arc::ptr_eq(&first, &v.config(&key, false).unwrap()));
    write(dir.path(), "d.crl", b"half written");
    v.refresh([&key]);
    assert!(Arc::ptr_eq(&first, &v.config(&key, false).unwrap()));
    let notes = v.notes(SystemTime::now());
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0]
            .1
            .contains("is not a CRL; the last good files stay in force"),
        "{notes:?}"
    );
    write(dir.path(), "d.crl", &crl(&root, &[9], soon()));
    v.refresh([&key]);
    assert!(!Arc::ptr_eq(&first, &v.config(&key, false).unwrap()));
    assert!(v.notes(SystemTime::now()).is_empty());
    let later = SystemTime::now() + Duration::from_secs(8 * 86400);
    assert_eq!(v.notes(later).len(), 1);
    v.refresh([]);
    assert!(v.config(&key, false).is_none());
}

#[test]
fn a_site_whose_files_were_never_good_gets_no_config() {
    let dir = tempfile::tempdir().unwrap();
    let key = ClientAuth {
        ca: write(dir.path(), "ca.pem", b"junk"),
        crl: None,
    };
    let resolver: Arc<dyn ResolvesServerCert> = Arc::new(crate::tls::Fixed::new(
        crate::tls::files::tests::certified_for("x.example.com"),
    ));
    let v = Verifiers::new(resolver);
    v.refresh([&key]);
    assert!(v.config(&key, false).is_none());
    assert!(v.verifier(&key).is_none());
    let notes = v.notes(SystemTime::now());
    assert!(
        notes[0]
            .1
            .contains("visitors are refused until it is fixed"),
        "{notes:?}"
    );
}
