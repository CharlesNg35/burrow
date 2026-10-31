use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, CertificateRevocationListParams, DistinguishedName,
    DnType, IsCa, Issuer, KeyIdMethod, KeyPair, KeyUsagePurpose, RevokedCertParams, SerialNumber,
};
use time::OffsetDateTime;

pub struct DeviceCa {
    pub params: CertificateParams,
    pub key: KeyPair,
    pub pem: String,
}

pub fn device_ca(name: &str, parent: Option<&DeviceCa>) -> DeviceCa {
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
    DeviceCa {
        pem: cert.pem(),
        params,
        key,
    }
}

pub fn crl(by: &DeviceCa, revoked: &[u64], next: OffsetDateTime) -> Vec<u8> {
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

pub fn next_week() -> OffsetDateTime {
    OffsetDateTime::now_utc() + time::Duration::days(7)
}

pub fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}
