use rcgen::{
    BasicConstraints, CertificateParams, CertificateRevocationListParams, IsCa, Issuer,
    KeyIdMethod, KeyPair, KeyUsagePurpose, SerialNumber, SignatureAlgorithm,
};

use super::*;

const NOW: u64 = 1_800_000_000;

fn at(secs: u64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(secs as i64).unwrap()
}

fn ca_with(
    alg: &'static SignatureAlgorithm,
    name: &str,
    usages: Vec<KeyUsagePurpose>,
) -> (String, Issuer<'static, KeyPair>) {
    let key = KeyPair::generate_for(alg).unwrap();
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.distinguished_name.push(rcgen::DnType::CommonName, name);
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = usages;
    p.not_before = at(NOW - 86_400);
    p.not_after = at(NOW + 365 * 86_400);
    let cert = p.self_signed(&key).unwrap();
    (cert.pem(), Issuer::new(p, key))
}

fn ca(alg: &'static SignatureAlgorithm, name: &str) -> (String, Issuer<'static, KeyPair>) {
    ca_with(
        alg,
        name,
        vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign],
    )
}

fn leaf(names: &[&str], issuer: &Issuer<'_, KeyPair>, until: u64) -> (String, String) {
    let key = KeyPair::generate().unwrap();
    let mut p =
        CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
    p.not_before = at(NOW - 3600);
    p.not_after = at(until);
    let cert = p.signed_by(&key, issuer).unwrap();
    (cert.pem(), key.serialize_pem())
}

fn crl(issuer: &Issuer<'_, KeyPair>) -> String {
    CertificateRevocationListParams {
        this_update: at(NOW - 3600),
        next_update: at(NOW + 86_400),
        crl_number: SerialNumber::from(1u64),
        issuing_distribution_point: None,
        revoked_certs: Vec::new(),
        key_identifier_method: KeyIdMethod::Sha256,
    }
    .signed_by(issuer)
    .unwrap()
    .pem()
    .unwrap()
}

fn up<'a>(kind: Kind, pem: &'a str, hosts: &'a [String], cas: &'a str) -> Upload<'a> {
    Upload {
        kind,
        pem: pem.as_bytes(),
        hosts,
        cas: cas.as_bytes(),
    }
}

fn err(u: &Upload<'_>) -> String {
    check(u, NOW).unwrap_err()
}

fn hosts(list: &[&str]) -> Vec<String> {
    list.iter().map(|h| h.to_string()).collect()
}

#[test]
fn a_good_pair_is_stored_once_under_its_hash_and_only_burrow_ids_resolve() {
    let (_, issuer) = ca(&rcgen::PKCS_ECDSA_P256_SHA256, "Test CA");
    let (cert, key) = leaf(&["nas.example.com"], &issuer, NOW + 90 * 86_400);
    let pem = format!("{cert}{key}");
    let h = hosts(&["nas.example.com"]);
    let state = tempfile::tempdir().unwrap();
    let writes = std::cell::Cell::new(0);
    let write = |p: &Path, b: &[u8]| {
        writes.set(writes.get() + 1);
        crate::tls::acme::storage::write_private(p, b)
    };
    let first = store(state.path(), &up(Kind::Pair, &pem, &h, ""), NOW, &write).unwrap();
    assert!(
        valid_id(&first.id) && first.id.starts_with("pair-"),
        "{}",
        first.id
    );
    assert_eq!(first.described.names, vec!["nas.example.com"]);
    assert!(
        first.described.issuer.contains("Test CA"),
        "{}",
        first.described.issuer
    );
    assert_eq!(first.described.sha256.len(), 95);
    let again = store(state.path(), &up(Kind::Pair, &pem, &h, ""), NOW, &write).unwrap();
    assert_eq!((again.id.clone(), writes.get()), (first.id.clone(), 1));
    let file = path(state.path(), &first.id).unwrap();
    assert_eq!(
        load(state.path(), &first.id).unwrap().as_slice(),
        pem.as_bytes()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let dir = std::fs::metadata(folder(state.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir & 0o777, 0o700);
    }
    assert!(
        crate::tls::files::load(&file, &file).is_ok(),
        "one file serves as cert and key"
    );
    for bad in [
        "../x",
        "pair-0123",
        "PAIR-0123456789abcdef",
        "key-0123456789abcdef",
        "pair-0123456789ABCDEF",
    ] {
        assert!(path(state.path(), bad).is_none(), "{bad}");
    }
    assert!(load(state.path(), "pair-0123456789abcdef").is_err());
    std::fs::write(
        folder(state.path()).join("pair-00000000000000aa.pem"),
        "old",
    )
    .unwrap();
    std::fs::write(folder(state.path()).join("notes.txt"), "mine").unwrap();
    assert_eq!(prune(state.path(), std::slice::from_ref(&first.id)), 1);
    assert!(file.exists());
    assert!(folder(state.path()).join("notes.txt").exists());
}

#[test]
fn a_pair_that_does_not_fit_is_refused_with_its_reason() {
    let (_, issuer) = ca(&rcgen::PKCS_ECDSA_P256_SHA256, "Test CA");
    let (cert, _) = leaf(&["nas.example.com"], &issuer, NOW + 90 * 86_400);
    let (_, other_key) = leaf(&["nas.example.com"], &issuer, NOW + 90 * 86_400);
    let h = hosts(&["nas.example.com"]);
    let mismatched = format!("{cert}{other_key}");
    assert!(
        err(&up(Kind::Pair, &mismatched, &h, ""))
            .starts_with("the key does not fit the certificate")
    );
    assert_eq!(
        err(&up(Kind::Pair, &cert, &h, "")),
        "needs exactly one private key, and holds 0"
    );
    let (cert, key) = leaf(&["*.example.com"], &issuer, NOW + 90 * 86_400);
    let wild = format!("{cert}{key}");
    assert!(
        check(
            &up(
                Kind::Pair,
                &wild,
                &hosts(&["a.example.com", "*.example.com"]),
                ""
            ),
            NOW
        )
        .is_ok()
    );
    assert_eq!(
        err(&up(Kind::Pair, &wild, &hosts(&["a.b.example.com"]), "")),
        "does not cover \"a.b.example.com\"; it names *.example.com"
    );
    let (cert, key) = leaf(&["nas.example.com"], &issuer, NOW - 60);
    assert_eq!(
        err(&up(Kind::Pair, &format!("{cert}{key}"), &h, "")),
        "the certificate has expired"
    );
    let (cert, key) = leaf(&["nas.example.com"], &issuer, NOW + 3 * 86_400);
    let (warnings, _) = check(&up(Kind::Pair, &format!("{cert}{key}"), &h, ""), NOW).unwrap();
    assert_eq!(warnings, vec!["the certificate expires in 3 days"]);
}

#[test]
fn encrypted_keys_foreign_blocks_and_oversize_files_are_refused_before_parsing() {
    let encrypted =
        "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIB\n-----END ENCRYPTED PRIVATE KEY-----\n";
    assert!(
        err(&up(Kind::Pair, encrypted, &[], ""))
            .starts_with("holds an encrypted key; remove its passphrase")
    );
    let public = "-----BEGIN PUBLIC KEY-----\nMIIB\n-----END PUBLIC KEY-----\n";
    assert_eq!(
        err(&up(Kind::Ca, public, &[], "")),
        "holds a \"PUBLIC KEY\" block, which burrow does not take here"
    );
    for (kind, limit) in [
        (Kind::Pair, PAIR_LIMIT),
        (Kind::Ca, CA_LIMIT),
        (Kind::Crl, CRL_LIMIT),
    ] {
        let big = "A".repeat(limit + 1);
        assert!(
            err(&up(kind, &big, &[], "")).starts_with(&format!("is {} bytes;", limit + 1)),
            "{kind:?}"
        );
    }
    assert_eq!(
        err(&up(Kind::Pair, "not pem at all", &[], "")),
        "holds no certificate"
    );
}

#[test]
fn a_ca_upload_holds_only_ca_certificates_that_may_sign() {
    let (good, _) = ca(&rcgen::PKCS_ECDSA_P256_SHA256, "Corp CA");
    let (_, d) = check(&up(Kind::Ca, &good, &[], ""), NOW).unwrap();
    assert!(d.names[0].contains("Corp CA"), "{:?}", d.names);
    let (_, issuer) = ca(&rcgen::PKCS_ECDSA_P256_SHA256, "Corp CA");
    let (plain, _) = leaf(&["nas.example.com"], &issuer, NOW + 86_400);
    assert!(
        err(&up(Kind::Ca, &plain, &[], "")).starts_with("certificate 1 "),
        "a leaf is no CA"
    );
    let (no_sign, _) = ca_with(
        &rcgen::PKCS_ECDSA_P256_SHA256,
        "Odd CA",
        vec![KeyUsagePurpose::CrlSign],
    );
    assert_eq!(
        err(&up(Kind::Ca, &format!("{good}{no_sign}"), &[], "")),
        "certificate 2 may not sign certificates (keyUsage has no keyCertSign)"
    );
}

#[test]
fn a_crl_is_taken_only_when_one_of_the_cas_signed_it() {
    let (root_pem, root) = ca(&rcgen::PKCS_ECDSA_P384_SHA384, "Root CA");
    let (inter_pem, inter) = ca(&rcgen::PKCS_ECDSA_P256_SHA256, "Intermediate CA");
    let (ed_pem, ed) = ca(&rcgen::PKCS_ED25519, "Ed CA");
    let cas = format!("{root_pem}{inter_pem}{ed_pem}");
    let both = format!("{}{}{}", crl(&root), crl(&inter), crl(&ed));
    let (_, d) = check(&up(Kind::Crl, &both, &[], &cas), NOW).unwrap();
    assert!(d.issuer.contains("Root CA"), "{}", d.issuer);
    let (_, stranger) = ca(&rcgen::PKCS_ECDSA_P256_SHA256, "Stranger CA");
    assert_eq!(
        err(&up(Kind::Crl, &crl(&stranger), &[], &cas)),
        "CRL 1 is not signed by any CA in client_auth.ca (its issuer: CN=Stranger CA)"
    );
    let (_, impostor) = ca(&rcgen::PKCS_ECDSA_P256_SHA256, "Intermediate CA");
    assert!(err(&up(Kind::Crl, &crl(&impostor), &[], &cas)).starts_with("CRL 1 is not signed"));
    let good = crl(&inter);
    let mut der = CertificateRevocationListDer::from_pem_slice(good.as_bytes())
        .unwrap()
        .to_vec();
    let last = der.len() - 1;
    der[last] ^= 1;
    assert!(
        !crl::signed_by(
            &der,
            &CertificateDer::from_pem_slice(inter_pem.as_bytes()).unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        err(&up(Kind::Crl, &good, &[], "")),
        "needs client_auth.ca first: a CRL is checked against its CAs"
    );
}
