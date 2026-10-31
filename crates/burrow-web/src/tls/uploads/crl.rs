use aws_lc_rs::signature::{self, UnparsedPublicKey, VerificationAlgorithm};
use x509_cert::certificate::{CertificateInner, Raw};
use x509_cert::crl::CertificateList;
use x509_cert::der::{Decode, Encode, Header, Reader, SliceReader};
use x509_cert::spki::ObjectIdentifier;

const ECDSA_SHA256: &str = "1.2.840.10045.4.3.2";
const ECDSA_SHA384: &str = "1.2.840.10045.4.3.3";
const RSA_SHA256: &str = "1.2.840.113549.1.1.11";
const RSA_SHA384: &str = "1.2.840.113549.1.1.12";
const RSA_SHA512: &str = "1.2.840.113549.1.1.13";
const ED25519: &str = "1.3.101.112";
const EC_KEY: &str = "1.2.840.10045.2.1";
const P256: &str = "1.2.840.10045.3.1.7";
const P384: &str = "1.3.132.0.34";

fn oid(text: &str) -> ObjectIdentifier {
    ObjectIdentifier::new_unwrap(text)
}

fn algorithm(
    sig: ObjectIdentifier,
    key: ObjectIdentifier,
    curve: Option<ObjectIdentifier>,
) -> Option<&'static dyn VerificationAlgorithm> {
    let ec = key == oid(EC_KEY);
    let on = |c: &str| ec && curve == Some(oid(c));
    let found: &'static dyn VerificationAlgorithm = match sig {
        s if s == oid(ECDSA_SHA256) && on(P256) => &signature::ECDSA_P256_SHA256_ASN1,
        s if s == oid(ECDSA_SHA256) && on(P384) => &signature::ECDSA_P384_SHA256_ASN1,
        s if s == oid(ECDSA_SHA384) && on(P256) => &signature::ECDSA_P256_SHA384_ASN1,
        s if s == oid(ECDSA_SHA384) && on(P384) => &signature::ECDSA_P384_SHA384_ASN1,
        s if s == oid(RSA_SHA256) && !ec => &signature::RSA_PKCS1_2048_8192_SHA256,
        s if s == oid(RSA_SHA384) && !ec => &signature::RSA_PKCS1_2048_8192_SHA384,
        s if s == oid(RSA_SHA512) && !ec => &signature::RSA_PKCS1_2048_8192_SHA512,
        s if s == oid(ED25519) && key == oid(ED25519) => &signature::ED25519,
        _ => return None,
    };
    Some(found)
}

fn signed_part(der: &[u8]) -> Option<&[u8]> {
    let mut r = SliceReader::new(der).ok()?;
    Header::decode(&mut r).ok()?;
    r.tlv_bytes().ok()
}

pub fn signed_by(crl_der: &[u8], ca_der: &[u8]) -> Result<bool, String> {
    let crl =
        CertificateList::<Raw>::from_der(crl_der).map_err(|e| format!("is not a CRL ({e})"))?;
    let ca = CertificateInner::<Raw>::from_der(ca_der).map_err(|e| e.to_string())?;
    let tbs = ca.tbs_certificate();
    let same_name = tbs.subject().to_der().ok() == crl.tbs_cert_list.issuer.to_der().ok();
    if !same_name {
        return Ok(false);
    }
    let spki = tbs.subject_public_key_info();
    let curve = spki
        .algorithm
        .parameters
        .as_ref()
        .and_then(|p| p.decode_as::<ObjectIdentifier>().ok());
    let alg = algorithm(crl.signature_algorithm.oid, spki.algorithm.oid, curve).ok_or_else(|| {
        format!(
            "is signed with {}, which burrow does not check (use ECDSA P-256/P-384, RSA PKCS#1 SHA-256/384/512 or Ed25519)",
            crl.signature_algorithm.oid
        )
    })?;
    let message =
        signed_part(crl_der).ok_or_else(|| "has an unreadable signed part".to_string())?;
    let sig = crl
        .signature
        .as_bytes()
        .ok_or_else(|| "has an unreadable signature".to_string())?;
    let key = spki
        .subject_public_key
        .as_bytes()
        .ok_or_else(|| "names a CA whose public key is unreadable".to_string())?;
    Ok(UnparsedPublicKey::new(alg, key)
        .verify(message, sig)
        .is_ok())
}
