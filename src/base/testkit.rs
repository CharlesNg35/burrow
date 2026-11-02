#![cfg(test)]

use std::path::Path;

pub(crate) fn cert_pair(dir: &Path, stem: &str, names: &[&str]) {
    let key = rcgen::KeyPair::generate().unwrap();
    let names = names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    let cert = rcgen::CertificateParams::new(names)
        .unwrap()
        .self_signed(&key)
        .unwrap();
    std::fs::write(dir.join(format!("{stem}.pem")), cert.pem()).unwrap();
    std::fs::write(dir.join(format!("{stem}.key")), key.serialize_pem()).unwrap();
}
