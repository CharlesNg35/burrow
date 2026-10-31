pub struct Cert {
    pub cert_pem: String,
    pub key_pem: String,
}

fn self_signed(names: &[&str], ca: bool) -> Cert {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params =
        rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
            .unwrap();
    if ca {
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    }
    let c = params.self_signed(&key).unwrap();
    Cert {
        cert_pem: c.pem(),
        key_pem: key.serialize_pem(),
    }
}

pub fn cert(names: &[&str]) -> Cert {
    self_signed(names, false)
}

pub fn ca_flagged(names: &[&str]) -> Cert {
    self_signed(names, true)
}
