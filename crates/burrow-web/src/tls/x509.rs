use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use x509_cert::certificate::{CertificateInner, Raw};
use x509_cert::der::Decode;
use x509_cert::ext::pkix::AuthorityKeyIdentifier;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub not_before: u64,
    pub not_after: u64,
    pub ari_id: Option<String>,
}

pub fn fingerprint(bytes: &[u8]) -> String {
    aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn short_hex(bytes: &[u8], n: usize) -> String {
    aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes).as_ref()[..n]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn facts(der: &[u8]) -> Option<Facts> {
    let cert = CertificateInner::<Raw>::from_der(der).ok()?;
    let tbs = cert.tbs_certificate();
    let validity = tbs.validity();
    let aki = tbs
        .get_extension::<AuthorityKeyIdentifier>()
        .ok()
        .flatten()
        .and_then(|(_, a)| a.key_identifier);
    let ari_id = aki.map(|k| {
        format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(k.as_bytes()),
            URL_SAFE_NO_PAD.encode(tbs.serial_number().as_bytes())
        )
    });
    Some(Facts {
        not_before: validity.not_before.to_unix_duration().as_secs(),
        not_after: validity.not_after.to_unix_duration().as_secs(),
        ari_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ari_id_matches_the_rfc_9773_example_with_the_leading_zero_of_the_serial() {
        let aki = vec![
            0x69, 0x88, 0x5B, 0x6B, 0x87, 0x46, 0x40, 0x41, 0xE1, 0xB3, 0x7B, 0x84, 0x7B, 0xA0,
            0xAE, 0x2C, 0xDE, 0x01, 0xC8, 0xD4,
        ];
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca.key_identifier_method = rcgen::KeyIdMethod::PreSpecified(aki);
        let issuer = rcgen::Issuer::new(ca, ca_key);
        let key = rcgen::KeyPair::generate().unwrap();
        let mut leaf = rcgen::CertificateParams::new(vec!["example.com".into()]).unwrap();
        leaf.use_authority_key_identifier_extension = true;
        leaf.serial_number = Some(rcgen::SerialNumber::from_slice(&[0x87, 0x65, 0x43, 0x21]));
        let cert = leaf.signed_by(&key, &issuer).unwrap();
        let got = facts(cert.der()).expect("the certificate parses");
        assert_eq!(
            got.ari_id.as_deref(),
            Some("aYhba4dGQEHhs3uEe6CuLN4ByNQ.AIdlQyE")
        );
    }

    #[test]
    fn the_dates_come_from_the_certificate() {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["a.example.com".into()]).unwrap();
        params.not_before = rcgen::date_time_ymd(2026, 1, 2);
        params.not_after = rcgen::date_time_ymd(2026, 4, 2);
        let cert = params.self_signed(&key).unwrap();
        let got = facts(cert.der()).unwrap();
        assert_eq!(got.not_before, 1_767_312_000);
        assert_eq!(got.not_after, 1_775_088_000);
        assert_eq!(got.ari_id, None);
        assert_eq!(facts(b"not a certificate"), None);
    }
}
