mod crl;

use std::path::{Path, PathBuf};

use tokio_rustls::rustls::pki_types::pem::{PemObject, SectionKind};
use tokio_rustls::rustls::pki_types::{
    CertificateDer, CertificateRevocationListDer, PrivateKeyDer,
};
use x509_cert::certificate::{CertificateInner, Raw};
use x509_cert::der::Decode;
use x509_cert::ext::pkix::{BasicConstraints, KeyUsage};
use zeroize::Zeroizing;

use super::files;

pub use burrow_syntax::upload::DRAFT_SECS;

pub const PAIR_LIMIT: usize = (64 + 16) * 1024;
pub const CA_LIMIT: usize = 256 * 1024;
pub const CRL_LIMIT: usize = 1024 * 1024;
const MAX_CHAIN: usize = 10;
const MAX_CAS: usize = 100;
const MAX_CRLS: usize = 16;

pub use burrow_syntax::upload::Kind;

fn limit(kind: Kind) -> usize {
    match kind {
        Kind::Pair => PAIR_LIMIT,
        Kind::Ca => CA_LIMIT,
        Kind::Crl => CRL_LIMIT,
    }
}

pub struct Upload<'a> {
    pub kind: Kind,
    pub pem: &'a [u8],
    pub hosts: &'a [String],
    pub cas: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Described {
    pub names: Vec<String>,
    pub issuer: String,
    pub not_after: Option<u64>,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub id: String,
    pub warnings: Vec<String>,
    pub described: Described,
}

pub type Write<'a> = &'a dyn Fn(&Path, &[u8]) -> std::io::Result<()>;

pub fn folder(state: &Path) -> PathBuf {
    state.join("files")
}

pub fn valid_id(id: &str) -> bool {
    burrow_syntax::upload::kind_of(id).is_some()
}

pub fn path(state: &Path, id: &str) -> Option<PathBuf> {
    valid_id(id).then(|| folder(state).join(format!("{id}.pem")))
}

pub fn id_of(kind: Kind, bytes: &[u8]) -> String {
    format!("{}-{}", kind.prefix(), super::x509::short_hex(bytes, 8))
}

pub fn store(state: &Path, up: &Upload<'_>, now: u64, write: Write<'_>) -> Result<Stored, String> {
    let (warnings, described) = check(up, now)?;
    let id = id_of(up.kind, up.pem);
    let file = path(state, &id).ok_or_else(|| "burrow made an unusable name".to_string())?;
    let same = std::fs::read(&file).is_ok_and(|old| Zeroizing::new(old).as_slice() == up.pem);
    if !same {
        super::acme::storage::make_dir(&folder(state))
            .map_err(|e| format!("cannot make {}: {e}", folder(state).display()))?;
        write(&file, up.pem).map_err(|e| format!("cannot save {}: {e}", file.display()))?;
    }
    Ok(Stored {
        id,
        warnings,
        described,
    })
}

pub fn load(state: &Path, id: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let file = path(state, id).ok_or_else(|| format!("\"{id}\" is not an upload burrow made"))?;
    std::fs::read(&file)
        .map(Zeroizing::new)
        .map_err(|e| format!("cannot read the upload {id}: {e}"))
}

fn young(entry: &std::fs::DirEntry, now: u64) -> bool {
    entry
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .is_none_or(|t| t.as_secs() + DRAFT_SECS > now)
}

pub fn prune(state: &Path, keep: &[String], now: u64) -> usize {
    let Ok(list) = std::fs::read_dir(folder(state)) else {
        return 0;
    };
    let mut removed = 0;
    for entry in list.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".pem").filter(|id| valid_id(id)) else {
            continue;
        };
        if keep.iter().any(|k| k == id) || young(&entry, now) {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

type Section = (SectionKind, Zeroizing<Vec<u8>>);

fn sections(pem: &[u8]) -> Result<Vec<Section>, String> {
    let text = std::str::from_utf8(pem).map_err(|_| "is not a PEM text file".to_string())?;
    if text.contains("ENCRYPTED PRIVATE KEY") || text.contains("Proc-Type: 4,ENCRYPTED") {
        return Err("holds an encrypted key; remove its passphrase first, for example with openssl pkey -in key.pem -out plain.pem".into());
    }
    for line in text.lines().map(str::trim) {
        if let Some(label) = line
            .strip_prefix("-----BEGIN ")
            .and_then(|l| l.strip_suffix("-----"))
            && !matches!(
                label,
                "CERTIFICATE" | "PRIVATE KEY" | "RSA PRIVATE KEY" | "EC PRIVATE KEY" | "X509 CRL"
            )
        {
            return Err(format!(
                "holds a \"{label}\" block, which burrow does not take here"
            ));
        }
    }
    let mut out = Vec::new();
    for item in <(SectionKind, Vec<u8>) as PemObject>::pem_slice_iter(pem) {
        let (kind, der) = item.map_err(|e| format!("is not readable PEM ({e})"))?;
        out.push((kind, Zeroizing::new(der)));
    }
    Ok(out)
}

pub fn check(up: &Upload<'_>, now: u64) -> Result<(Vec<String>, Described), String> {
    let kind = up.kind;
    if up.pem.len() > limit(kind) {
        return Err(format!(
            "is {} bytes; {} may be at most {} KiB",
            up.pem.len(),
            kind.what(),
            limit(kind) / 1024
        ));
    }
    let parts = sections(up.pem)?;
    let certs: Vec<CertificateDer<'static>> = parts
        .iter()
        .filter(|(k, _)| *k == SectionKind::Certificate)
        .map(|(_, d)| CertificateDer::from(d.to_vec()))
        .collect();
    let sha256 = super::x509::fingerprint(up.pem);
    let (warnings, mut described) = match kind {
        Kind::Pair => pair(&parts, certs, up.hosts, now)?,
        Kind::Ca => cas(&certs, now)?,
        Kind::Crl => crls(&parts, up.cas)?,
    };
    described.sha256 = sha256;
    Ok((warnings, described))
}

fn describe(der: &[u8]) -> Described {
    let Ok(cert) = CertificateInner::<Raw>::from_der(der) else {
        return Described::default();
    };
    let tbs = cert.tbs_certificate();
    Described {
        names: webpki::EndEntityCert::try_from(&CertificateDer::from(der))
            .map(|ee| ee.valid_dns_names().map(str::to_string).collect())
            .unwrap_or_default(),
        issuer: tbs.issuer().to_string(),
        not_after: Some(tbs.validity().not_after.to_unix_duration().as_secs()),
        sha256: String::new(),
    }
}

fn dates(der: &[u8], now: u64, warnings: &mut Vec<String>, what: &str) -> Result<(), String> {
    let facts = super::x509::facts(der).ok_or_else(|| format!("{what} has no readable dates"))?;
    if facts.not_after <= now {
        return Err(format!("{what} has expired"));
    }
    if burrow_syntax::web::expires_soon(facts.not_after, now) {
        warnings.push(format!(
            "{what} expires in {}",
            burrow_syntax::duration::days_left(facts.not_after, now)
        ));
    }
    Ok(())
}

type Checked = (Vec<String>, Described);

fn pair(
    parts: &[Section],
    chain: Vec<CertificateDer<'static>>,
    hosts: &[String],
    now: u64,
) -> Result<Checked, String> {
    if chain.is_empty() {
        return Err("holds no certificate".into());
    }
    if chain.len() > MAX_CHAIN {
        return Err(format!(
            "holds {} certificates; a chain has at most {MAX_CHAIN}",
            chain.len()
        ));
    }
    let keys: Vec<&Zeroizing<Vec<u8>>> = parts
        .iter()
        .filter(|(k, _)| {
            matches!(
                k,
                SectionKind::PrivateKey | SectionKind::RsaPrivateKey | SectionKind::EcPrivateKey
            )
        })
        .map(|(_, d)| d)
        .collect();
    let [key] = keys.as_slice() else {
        return Err(format!(
            "needs exactly one private key, and holds {}",
            keys.len()
        ));
    };
    let private = PrivateKeyDer::try_from(key.to_vec())
        .map_err(|e| format!("holds an unreadable key ({e})"))?;
    let leaf_der = chain[0].to_vec();
    let ck = files::certified(chain, private)?;
    let mut warnings = Vec::new();
    dates(&leaf_der, now, &mut warnings, "the certificate")?;
    if let Some(h) = hosts.iter().find(|h| !files::covers(&ck, h)) {
        return Err(format!(
            "does not cover \"{h}\"; it names {}",
            files::dns_names(&ck).join(", ")
        ));
    }
    Ok((warnings, describe(&leaf_der)))
}

fn cas(certs: &[CertificateDer<'static>], now: u64) -> Result<Checked, String> {
    if certs.is_empty() {
        return Err("holds no certificate".into());
    }
    if certs.len() > MAX_CAS {
        return Err(format!(
            "holds {} certificates; a CA file has at most {MAX_CAS}",
            certs.len()
        ));
    }
    let mut warnings = Vec::new();
    for (i, der) in certs.iter().enumerate() {
        let what = format!("certificate {}", i + 1);
        webpki::anchor_from_trusted_cert(der)
            .map_err(|e| format!("{what} cannot be a CA ({e:?})"))?;
        let cert = CertificateInner::<Raw>::from_der(der)
            .map_err(|e| format!("{what} is unreadable ({e})"))?;
        let tbs = cert.tbs_certificate();
        let ca = tbs
            .get_extension::<BasicConstraints>()
            .ok()
            .flatten()
            .is_some_and(|(_, b)| b.ca);
        if !ca {
            return Err(format!(
                "{what} is not a CA certificate (basicConstraints cA is not set)"
            ));
        }
        let signs = tbs
            .get_extension::<KeyUsage>()
            .ok()
            .flatten()
            .is_none_or(|(_, k)| k.key_cert_sign());
        if !signs {
            return Err(format!(
                "{what} may not sign certificates (keyUsage has no keyCertSign)"
            ));
        }
        dates(der, now, &mut warnings, &what)?;
    }
    let mut d = describe(&certs[0]);
    if let Ok(cert) = CertificateInner::<Raw>::from_der(&certs[0]) {
        d.names = vec![cert.tbs_certificate().subject().to_string()];
    }
    Ok((warnings, d))
}

fn crls(parts: &[Section], cas_pem: &[u8]) -> Result<Checked, String> {
    let lists: Vec<CertificateRevocationListDer<'_>> = parts
        .iter()
        .filter(|(k, _)| *k == SectionKind::Crl)
        .map(|(_, d)| CertificateRevocationListDer::from(d.as_slice()))
        .collect();
    if lists.is_empty() {
        return Err("holds no CRL".into());
    }
    if lists.len() > MAX_CRLS {
        return Err(format!("holds {} CRLs; at most {MAX_CRLS}", lists.len()));
    }
    let cas: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cas_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("the CA file is unreadable ({e})"))?;
    if cas.is_empty() {
        return Err("needs client_auth.ca first: a CRL is checked against its CAs".into());
    }
    let mut d = Described::default();
    for (i, list) in lists.iter().enumerate() {
        let what = format!("CRL {}", i + 1);
        webpki::BorrowedCertRevocationList::from_der(list.as_ref())
            .map_err(|e| format!("{what} is unreadable ({e:?})"))?;
        let mut found = false;
        for ca in &cas {
            if crl::signed_by(list.as_ref(), ca).map_err(|e| format!("{what} {e}"))? {
                found = true;
                break;
            }
        }
        if !found {
            return Err(format!(
                "{what} is not signed by any CA in client_auth.ca (its issuer: {})",
                issuer_text(list.as_ref())
            ));
        }
        if d.issuer.is_empty() {
            d.issuer = issuer_text(list.as_ref());
        }
    }
    Ok((Vec::new(), d))
}

fn issuer_text(crl_der: &[u8]) -> String {
    x509_cert::crl::CertificateList::<Raw>::from_der(crl_der)
        .map(|c| c.tbs_cert_list.issuer.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
