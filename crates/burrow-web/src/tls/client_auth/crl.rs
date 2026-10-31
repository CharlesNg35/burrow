use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, CertificateRevocationListDer};
use x509_cert::certificate::{CertificateInner, Raw};
use x509_cert::crl::CertificateList;
use x509_cert::der::asn1::{AnyRef, BitStringRef};
use x509_cert::der::{Decode, Encode, Reader, SliceReader, Tag, Tagged};
use x509_cert::ext::pkix::BasicConstraints;
use x509_cert::name::Name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientAuthProblem {
    CaUnreadable(String),
    NotCa,
    CrlUnreadable(String),
    NotCrl,
    CrlV1,
    NotSignedByCa { issuer: String },
}

impl fmt::Display for ClientAuthProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientAuthProblem::CaUnreadable(e) | ClientAuthProblem::CrlUnreadable(e) => {
                write!(f, "cannot read {e}")
            }
            ClientAuthProblem::NotCa => write!(f, "is not a CA certificate"),
            ClientAuthProblem::NotCrl => write!(f, "is not a CRL"),
            ClientAuthProblem::CrlV1 => write!(
                f,
                "is an old version 1 CRL without extensions, which burrow cannot check; with openssl, create a crlnumber file next to the CA (echo 01 > crlnumber) and run openssl ca -gencrl again"
            ),
            ClientAuthProblem::NotSignedByCa { issuer } => write!(
                f,
                "holds a CRL from \"{issuer}\", which is not signed by any CA in client_auth.ca"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrlNote {
    Expired {
        issuer: String,
        next_update: SystemTime,
    },
}

impl fmt::Display for CrlNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CrlNote::Expired {
                issuer,
                next_update,
            } => write!(
                f,
                "the CRL from \"{issuer}\" expired on {}; it still applies, publish a new one",
                httpdate::fmt_http_date(*next_update)
            ),
        }
    }
}

pub(crate) struct Ca {
    pub der: CertificateDer<'static>,
    subject: Name,
    spki_alg: Vec<u8>,
    spki_key: Vec<u8>,
}

fn is_pem(bytes: &[u8]) -> bool {
    bytes.windows(11).any(|w| w == b"-----BEGIN ")
}

fn contents(der: &[u8]) -> Option<Vec<u8>> {
    Some(AnyRef::from_der(der).ok()?.value().to_vec())
}

fn ca(der: CertificateDer<'static>) -> Option<Ca> {
    let cert = CertificateInner::<Raw>::from_der(der.as_ref()).ok()?;
    let tbs = cert.tbs_certificate();
    let is_ca = tbs
        .get_extension::<BasicConstraints>()
        .ok()
        .flatten()
        .is_some_and(|(_, bc)| bc.ca);
    if !is_ca {
        return None;
    }
    let spki = tbs.subject_public_key_info();
    Some(Ca {
        subject: tbs.subject().clone(),
        spki_alg: contents(&spki.algorithm.to_der().ok()?)?,
        spki_key: spki.subject_public_key.raw_bytes().to_vec(),
        der,
    })
}

pub(crate) fn read_cas(bytes: &[u8]) -> Result<Vec<Ca>, ClientAuthProblem> {
    let ders: Vec<CertificateDer<'static>> = if is_pem(bytes) {
        CertificateDer::pem_slice_iter(bytes)
            .collect::<Result<_, _>>()
            .map_err(|_| ClientAuthProblem::NotCa)?
    } else {
        vec![CertificateDer::from(bytes.to_vec())]
    };
    if ders.is_empty() {
        return Err(ClientAuthProblem::NotCa);
    }
    ders.into_iter()
        .map(|d| ca(d).ok_or(ClientAuthProblem::NotCa))
        .collect()
}

pub(crate) fn read_crls(
    bytes: &[u8],
) -> Result<Vec<CertificateRevocationListDer<'static>>, ClientAuthProblem> {
    let crls: Vec<CertificateRevocationListDer<'static>> = if is_pem(bytes) {
        CertificateRevocationListDer::pem_slice_iter(bytes)
            .collect::<Result<_, _>>()
            .map_err(|_| ClientAuthProblem::NotCrl)?
    } else {
        vec![CertificateRevocationListDer::from(bytes.to_vec())]
    };
    if crls.is_empty() {
        return Err(ClientAuthProblem::NotCrl);
    }
    Ok(crls)
}

struct Signed<'a> {
    tbs: &'a [u8],
    alg: &'a [u8],
    signature: &'a [u8],
}

fn signed(der: &[u8]) -> Option<Signed<'_>> {
    let outer = AnyRef::from_der(der).ok()?;
    if outer.tag() != Tag::Sequence {
        return None;
    }
    let mut r = SliceReader::new(outer.value()).ok()?;
    let tbs = r.tlv_bytes().ok()?;
    let alg = AnyRef::decode(&mut r).ok()?.value();
    let signature = BitStringRef::decode(&mut r).ok()?.raw_bytes();
    r.is_finished().then_some(Signed {
        tbs,
        alg,
        signature,
    })
}

fn version_one(tbs: &[u8]) -> bool {
    let Some(inner) = AnyRef::from_der(tbs).ok() else {
        return false;
    };
    SliceReader::new(inner.value())
        .ok()
        .and_then(|mut r| AnyRef::decode(&mut r).ok())
        .is_some_and(|first| first.tag() == Tag::Sequence)
}

fn signed_by(s: &Signed<'_>, ca: &Ca) -> bool {
    crate::tls::provider()
        .signature_verification_algorithms
        .all
        .iter()
        .filter(|a| a.signature_alg_id().as_ref() == s.alg)
        .filter(|a| a.public_key_alg_id().as_ref() == ca.spki_alg.as_slice())
        .any(|a| a.verify_signature(&ca.spki_key, s.tbs, s.signature).is_ok())
}

fn unix(t: &x509_cert::time::Time) -> SystemTime {
    UNIX_EPOCH + t.to_unix_duration()
}

pub(crate) fn check_crl(der: &[u8], cas: &[Ca]) -> Result<(String, SystemTime), ClientAuthProblem> {
    let s = signed(der).ok_or(ClientAuthProblem::NotCrl)?;
    if version_one(s.tbs) {
        return Err(ClientAuthProblem::CrlV1);
    }
    let crl = CertificateList::<Raw>::from_der(der).map_err(|_| ClientAuthProblem::NotCrl)?;
    let tbs = &crl.tbs_cert_list;
    if tbs.crl_extensions.is_none() {
        return Err(ClientAuthProblem::CrlV1);
    }
    webpki::BorrowedCertRevocationList::from_der(der).map_err(|_| ClientAuthProblem::NotCrl)?;
    let issuer = tbs.issuer.to_string();
    let signer = cas
        .iter()
        .filter(|c| c.subject == tbs.issuer)
        .any(|c| signed_by(&s, c));
    if !signer {
        return Err(ClientAuthProblem::NotSignedByCa { issuer });
    }
    let next = tbs
        .next_update
        .as_ref()
        .map(unix)
        .ok_or(ClientAuthProblem::NotCrl)?;
    Ok((issuer, next))
}
