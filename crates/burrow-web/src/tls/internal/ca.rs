use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    NameConstraints,
};
use serde::{Deserialize, Serialize};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::sign::CertifiedKey;

use super::Write;

pub const ROOT_SECS: u64 = 3600 * 86_400;
pub const INTERMEDIATE_SECS: u64 = 7 * 86_400;
pub const LEAF_SECS: u64 = 12 * 3600;
pub const SKEW_SECS: u64 = 300;
pub const ROOT_WARNING_SECS: u64 = 365 * 86_400;

pub const PERMITTED_NAMES: [&str; 5] = ["localhost", "local", "internal", "home.arpa", "lan"];
pub const PERMITTED_NETS: [(&str, u8); 9] = [
    ("10.0.0.0", 8),
    ("172.16.0.0", 12),
    ("192.168.0.0", 16),
    ("127.0.0.0", 8),
    ("169.254.0.0", 16),
    ("100.64.0.0", 10),
    ("::1", 128),
    ("fc00::", 7),
    ("fe80::", 10),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Meta {
    root: String,
    intermediate: String,
    created: u64,
}

pub struct Signer {
    pub der: CertificateDer<'static>,
    issuer: Issuer<'static, KeyPair>,
    pub not_before: u64,
    pub not_after: u64,
}

pub struct Ca {
    dir: PathBuf,
    meta: Meta,
    pub root: CertificateDer<'static>,
    pub root_not_before: u64,
    pub root_not_after: u64,
    pub intermediate: Signer,
}

fn time(secs: u64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(i64::try_from(secs).unwrap_or(i64::MAX))
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

fn constraints() -> NameConstraints {
    let mut permitted: Vec<GeneralSubtree> = PERMITTED_NAMES
        .iter()
        .map(|n| GeneralSubtree::DnsName((*n).to_string()))
        .collect();
    for (addr, prefix) in PERMITTED_NETS {
        let ip: IpAddr = addr.parse().unwrap_or(IpAddr::from([0u8; 4]));
        permitted.push(GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(
            ip, prefix,
        )));
    }
    NameConstraints {
        permitted_subtrees: permitted,
        excluded_subtrees: Vec::new(),
    }
}

fn ca_params(cn: &str, path_len: u8, not_before: u64, not_after: u64) -> CertificateParams {
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, cn);
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(path_len));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    p.name_constraints = Some(constraints());
    p.use_authority_key_identifier_extension = path_len == 0;
    p.not_before = time(not_before);
    p.not_after = time(not_after);
    p
}

fn dates(der: &[u8]) -> Result<(u64, u64), String> {
    crate::tls::x509::facts(der)
        .map(|f| (f.not_before, f.not_after))
        .ok_or_else(|| "the local CA certificate has no readable dates".to_string())
}

fn read_cert(path: &Path) -> Result<CertificateDer<'static>, String> {
    CertificateDer::from_pem_file(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn read_key(path: &Path) -> Result<KeyPair, String> {
    let text = zeroize::Zeroizing::new(
        std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?,
    );
    KeyPair::from_pem(&text).map_err(|e| format!("{} is not a usable key: {e}", path.display()))
}

fn save(write: Write<'_>, path: &Path, text: &str) -> Result<(), String> {
    write(path, text.as_bytes()).map_err(|e| format!("cannot save {}: {e}", path.display()))
}

pub fn fingerprint(der: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, der)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

impl Ca {
    fn path(dir: &Path, name: &str) -> PathBuf {
        dir.join(name)
    }

    pub fn exists(dir: &Path) -> bool {
        Self::path(dir, "root.pem").exists() || Self::path(dir, "ca.json").exists()
    }

    pub fn create(dir: &Path, now: u64, write: Write<'_>) -> Result<Ca, String> {
        crate::tls::acme::storage::make_dir(dir)
            .map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let stamp = time(now);
        let root = format!(
            "burrow local CA {:04}-{:02}-{:02} {:02}{:02}{:02}",
            stamp.year(),
            u8::from(stamp.month()),
            stamp.day(),
            stamp.hour(),
            stamp.minute(),
            stamp.second()
        );
        let meta = Meta {
            intermediate: format!("{root} intermediate"),
            root,
            created: now,
        };
        let key = KeyPair::generate().map_err(|e| format!("cannot make the CA key: {e}"))?;
        let params = ca_params(&meta.root, 1, now - SKEW_SECS.min(now), now + ROOT_SECS);
        let cert = params
            .self_signed(&key)
            .map_err(|e| format!("cannot make the CA certificate: {e}"))?;
        save(write, &Self::path(dir, "root.key"), &key.serialize_pem())?;
        save(write, &Self::path(dir, "root.pem"), &cert.pem())?;
        let json = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
        save(write, &Self::path(dir, "ca.json"), &json)?;
        let root = cert.der().clone();
        let (nb, na) = dates(&root)?;
        let issuer = Issuer::new(params, key);
        let intermediate = Self::sign_intermediate(dir, &meta, &issuer, now, na, write)?;
        Ok(Ca {
            dir: dir.to_path_buf(),
            meta,
            root,
            root_not_before: nb,
            root_not_after: na,
            intermediate,
        })
    }

    fn sign_intermediate(
        dir: &Path,
        meta: &Meta,
        root: &Issuer<'_, KeyPair>,
        now: u64,
        root_not_after: u64,
        write: Write<'_>,
    ) -> Result<Signer, String> {
        let key = KeyPair::generate().map_err(|e| format!("cannot make a key: {e}"))?;
        let until = (now + INTERMEDIATE_SECS).min(root_not_after);
        let params = ca_params(&meta.intermediate, 0, now - SKEW_SECS.min(now), until);
        let cert = params
            .signed_by(&key, root)
            .map_err(|e| format!("cannot sign the intermediate certificate: {e}"))?;
        save(
            write,
            &Self::path(dir, "intermediate.key"),
            &key.serialize_pem(),
        )?;
        save(write, &Self::path(dir, "intermediate.pem"), &cert.pem())?;
        let der = cert.der().clone();
        let (not_before, not_after) = dates(&der)?;
        Ok(Signer {
            der,
            issuer: Issuer::new(params, key),
            not_before,
            not_after,
        })
    }

    pub fn open(dir: &Path, now: u64, write: Write<'_>) -> Result<Ca, String> {
        let meta: Meta = std::fs::read(Self::path(dir, "ca.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or_else(|| format!("the local CA in {} has no readable ca.json", dir.display()))?;
        let root = read_cert(&Self::path(dir, "root.pem"))?;
        let (root_not_before, root_not_after) = dates(&root)?;
        let kept = Self::load_intermediate(dir, &meta);
        let intermediate = match kept {
            Some(s) if !due(s.not_before, s.not_after, now) => s,
            _ => Self::renew_intermediate(dir, &meta, root_not_after, now, write)?,
        };
        Ok(Ca {
            dir: dir.to_path_buf(),
            meta,
            root,
            root_not_before,
            root_not_after,
            intermediate,
        })
    }

    fn load_intermediate(dir: &Path, meta: &Meta) -> Option<Signer> {
        let der = read_cert(&Self::path(dir, "intermediate.pem")).ok()?;
        let key = read_key(&Self::path(dir, "intermediate.key")).ok()?;
        let (not_before, not_after) = dates(&der).ok()?;
        let params = ca_params(&meta.intermediate, 0, not_before, not_after);
        Some(Signer {
            der,
            issuer: Issuer::new(params, key),
            not_before,
            not_after,
        })
    }

    fn renew_intermediate(
        dir: &Path,
        meta: &Meta,
        root_not_after: u64,
        now: u64,
        write: Write<'_>,
    ) -> Result<Signer, String> {
        let key = read_key(&Self::path(dir, "root.key")).map_err(|e| {
            format!("{e}; the local CA's key is needed to renew its intermediate certificate, so restore {} from a backup", dir.join("root.key").display())
        })?;
        let params = ca_params(&meta.root, 1, 0, root_not_after);
        let root = Issuer::new(params, key);
        Self::sign_intermediate(dir, meta, &root, now, root_not_after, write)
    }

    pub fn refresh(&mut self, now: u64, write: Write<'_>) -> Result<bool, String> {
        if !due(
            self.intermediate.not_before,
            self.intermediate.not_after,
            now,
        ) {
            return Ok(false);
        }
        self.intermediate =
            Self::renew_intermediate(&self.dir, &self.meta, self.root_not_after, now, write)?;
        Ok(true)
    }

    pub fn subject(&self) -> &str {
        &self.meta.root
    }

    pub fn issue(&self, names: &[String], now: u64) -> Result<(Arc<CertifiedKey>, u64), String> {
        let mut p = CertificateParams::new(names.to_vec())
            .map_err(|e| format!("cannot name the certificate: {e}"))?;
        p.distinguished_name = DistinguishedName::new();
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.use_authority_key_identifier_extension = true;
        let not_after = (now + LEAF_SECS).min(self.intermediate.not_after);
        p.not_before = time(now - SKEW_SECS.min(now));
        p.not_after = time(not_after);
        let key = KeyPair::generate().map_err(|e| format!("cannot make a key: {e}"))?;
        let cert = p
            .signed_by(&key, &self.intermediate.issuer)
            .map_err(|e| format!("cannot sign the certificate: {e}"))?;
        let private = PrivateKeyDer::try_from(key.serialize_der())
            .map_err(|e| format!("cannot use the new key: {e}"))?;
        let chain = vec![cert.der().clone(), self.intermediate.der.clone()];
        let ck = crate::tls::files::certified(chain, private)?;
        Ok((Arc::new(ck), not_after))
    }
}

pub fn due(not_before: u64, not_after: u64, now: u64) -> bool {
    let life = not_after.saturating_sub(not_before);
    now + life / 5 >= not_after
}

pub fn leaf_renew_at(not_before: u64, not_after: u64) -> u64 {
    not_after - not_after.saturating_sub(not_before) / 3
}

pub fn leaf_due(not_before: u64, not_after: u64, now: u64) -> bool {
    now >= leaf_renew_at(not_before, not_after)
}
