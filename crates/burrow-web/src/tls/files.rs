use std::path::Path;
use std::sync::Arc;

use tokio_rustls::rustls::crypto::ring::default_provider;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio_rustls::rustls::sign::CertifiedKey;

pub fn load(cert: &Path, key: &Path) -> Result<Arc<CertifiedKey>, String> {
    let chain = CertificateDer::pem_file_iter(cert)
        .and_then(Iterator::collect::<Result<Vec<_>, _>>)
        .map_err(|e| format!("cannot read {}: {e}", cert.display()))?;
    let private = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| format!("cannot read {}: {e}", key.display()))?;
    certified(chain, private)
        .map(Arc::new)
        .map_err(|e| format!("{}: {e}", cert.display()))
}

pub fn certified(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<CertifiedKey, String> {
    if chain.is_empty() {
        return Err("holds no certificate".into());
    }
    let ck = CertifiedKey::from_der(chain, key, &default_provider())
        .map_err(|e| format!("the key does not fit the certificate ({e})"))?;
    ck.keys_match()
        .map_err(|e| format!("the key does not fit the certificate ({e})"))?;
    Ok(ck)
}

pub fn dns_names(ck: &CertifiedKey) -> Vec<String> {
    ck.cert
        .first()
        .and_then(|leaf| webpki::EndEntityCert::try_from(leaf).ok())
        .map(|ee| ee.valid_dns_names().map(str::to_ascii_lowercase).collect())
        .unwrap_or_default()
}

pub fn covers(ck: &CertifiedKey, name: &str) -> bool {
    let Some(leaf) = ck.cert.first() else {
        return false;
    };
    let Ok(ee) = webpki::EndEntityCert::try_from(leaf) else {
        return false;
    };
    if name.starts_with("*.") {
        return ee.valid_dns_names().any(|n| n.eq_ignore_ascii_case(name));
    }
    ServerName::try_from(name).is_ok_and(|n| ee.verify_is_valid_for_subject_name(&n).is_ok())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use super::*;

    pub(crate) fn certified_for(name: &str) -> Arc<CertifiedKey> {
        let dir = tempfile::tempdir().unwrap();
        let (c, k) = write_pair(dir.path(), "x", &[name]);
        load(&c, &k).unwrap()
    }

    pub(crate) fn write_pair(dir: &Path, stem: &str, names: &[&str]) -> (PathBuf, PathBuf) {
        let key = rcgen::KeyPair::generate().unwrap();
        let params =
            rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
                .unwrap();
        let cert = params.self_signed(&key).unwrap();
        let (c, k) = (
            dir.join(format!("{stem}.pem")),
            dir.join(format!("{stem}.key")),
        );
        std::fs::write(&c, cert.pem()).unwrap();
        std::fs::write(&k, key.serialize_pem()).unwrap();
        (c, k)
    }

    #[test]
    fn a_pem_pair_loads_and_its_names_are_covered_like_a_browser_checks_them() {
        let dir = tempfile::tempdir().unwrap();
        let names = ["*.example.com", "example.com", "203.0.113.10"];
        let (c, k) = write_pair(dir.path(), "a", &names);
        let ck = load(&c, &k).unwrap();
        for name in [
            "example.com",
            "x.example.com",
            "*.example.com",
            "203.0.113.10",
        ] {
            assert!(covers(&ck, name), "{name}");
        }
        for name in [
            "a.b.example.com",
            "example.org",
            "*.b.example.com",
            "203.0.113.11",
        ] {
            assert!(!covers(&ck, name), "{name}");
        }
        assert!(covers(&ck, "X.Example.COM"));
    }

    #[test]
    fn the_dns_names_of_a_file_are_read_lowercased_without_its_ip_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let (c, k) = write_pair(
            dir.path(),
            "a",
            &["*.Example.com", "a.example.org", "203.0.113.10"],
        );
        let ck = load(&c, &k).unwrap();
        assert_eq!(dns_names(&ck), ["*.example.com", "a.example.org"]);
    }

    #[test]
    fn a_wildcard_name_needs_that_exact_wildcard_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (c, k) = write_pair(dir.path(), "a", &["a.example.com", "b.example.com"]);
        let ck = load(&c, &k).unwrap();
        assert!(!covers(&ck, "*.example.com"));
    }

    #[test]
    fn a_key_of_another_certificate_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = write_pair(dir.path(), "a", &["a.example.com"]);
        let (_, other) = write_pair(dir.path(), "b", &["a.example.com"]);
        let e = load(&c, &other).unwrap_err();
        assert!(e.contains("the key does not fit the certificate"), "{e}");
    }

    #[test]
    fn missing_empty_and_garbage_files_say_what_is_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let (c, k) = write_pair(dir.path(), "a", &["a.example.com"]);
        let gone = dir.path().join("gone.pem");
        assert!(load(&gone, &k).unwrap_err().starts_with("cannot read"));
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "").unwrap();
        assert!(
            load(&empty, &k)
                .unwrap_err()
                .contains("holds no certificate")
        );
        let junk = dir.path().join("junk.key");
        std::fs::write(&junk, "not a key").unwrap();
        assert!(load(&c, &junk).unwrap_err().starts_with("cannot read"));
    }
}
