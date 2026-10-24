use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};

use tokio_rustls::rustls::pki_types::PrivateKeyDer;
use tokio_rustls::rustls::sign::CertifiedKey;

#[derive(Debug, Default)]
pub struct Challenges {
    alpn: RwLock<HashMap<String, Arc<CertifiedKey>>>,
    http: RwLock<HashMap<String, String>>,
}

impl Challenges {
    pub fn alpn(&self, name: &str) -> Option<Arc<CertifiedKey>> {
        let name = burrow_syntax::web::sni_name(name);
        self.alpn
            .read()
            .expect("challenge lock")
            .get(&name)
            .cloned()
    }

    pub fn http(&self, token: &str) -> Option<String> {
        if !valid_token(token) {
            return None;
        }
        self.http
            .read()
            .expect("challenge lock")
            .get(token)
            .cloned()
    }

    pub fn answer_alpn(&self, identifier: &str, digest: &[u8]) -> Result<(), String> {
        let key = alpn_certificate(identifier, digest)?;
        self.alpn
            .write()
            .expect("challenge lock")
            .insert(sni_for(identifier), Arc::new(key));
        Ok(())
    }

    pub fn answer_http(&self, token: &str, key_authorization: &str) -> Result<(), String> {
        if !valid_token(token) {
            return Err(format!("the CA sent an unusable HTTP-01 token {token:?}"));
        }
        self.http
            .write()
            .expect("challenge lock")
            .insert(token.to_string(), key_authorization.to_string());
        Ok(())
    }

    pub fn forget(&self, identifiers: &[String], tokens: &[String]) {
        let mut alpn = self.alpn.write().expect("challenge lock");
        for i in identifiers {
            alpn.remove(&sni_for(i));
        }
        let mut http = self.http.write().expect("challenge lock");
        for t in tokens {
            http.remove(t);
        }
    }
}

pub fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn sni_for(identifier: &str) -> String {
    match identifier.parse::<IpAddr>() {
        Ok(ip) => reverse_name(ip),
        Err(_) => identifier.to_ascii_lowercase(),
    }
}

pub fn reverse_name(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let [a, b, c, d] = v4.octets();
            format!("{d}.{c}.{b}.{a}.in-addr.arpa")
        }
        IpAddr::V6(v6) => {
            let mut out = String::with_capacity(72);
            for byte in v6.octets().iter().rev() {
                out.push_str(&format!("{:x}.{:x}.", byte & 0xf, byte >> 4));
            }
            out.push_str("ip6.arpa");
            out
        }
    }
}

fn alpn_certificate(identifier: &str, digest: &[u8]) -> Result<CertifiedKey, String> {
    let fail = |e: rcgen::Error| format!("cannot make the TLS-ALPN-01 certificate: {e}");
    let key = rcgen::KeyPair::generate().map_err(fail)?;
    let mut params = rcgen::CertificateParams::new(vec![identifier.to_string()]).map_err(fail)?;
    params.custom_extensions = vec![rcgen::CustomExtension::new_acme_identifier(digest)];
    let cert = params.self_signed(&key).map_err(fail)?;
    let private = PrivateKeyDer::try_from(key.serialize_der())
        .map_err(|e| format!("cannot use the TLS-ALPN-01 key: {e}"))?;
    let signing = tokio_rustls::rustls::crypto::ring::default_provider()
        .key_provider
        .load_private_key(private)
        .map_err(|e| format!("cannot use the TLS-ALPN-01 key: {e}"))?;
    Ok(CertifiedKey::new(vec![cert.der().clone()], signing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_addresses_answer_on_their_reverse_names() {
        assert_eq!(
            reverse_name("203.0.113.10".parse().unwrap()),
            "10.113.0.203.in-addr.arpa"
        );
        assert_eq!(
            reverse_name("2001:db8::567:89ab".parse().unwrap()),
            "b.a.9.8.7.6.5.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.8.b.d.0.1.0.0.2.ip6.arpa"
        );
        assert_eq!(
            reverse_name("::ffff:192.0.2.1".parse().unwrap()),
            "1.2.0.192.in-addr.arpa"
        );
        assert_eq!(sni_for("Blog.Example.com"), "blog.example.com");
    }

    #[test]
    fn only_base64url_tokens_are_answered() {
        let c = Challenges::default();
        c.answer_http("aB-_9", "aB-_9.thumb").unwrap();
        assert_eq!(c.http("aB-_9").as_deref(), Some("aB-_9.thumb"));
        assert!(c.answer_http("", "x").is_err());
        assert!(c.answer_http("../x", "x").is_err());
        assert_eq!(c.http(""), None);
        c.forget(&[], &["aB-_9".into()]);
        assert_eq!(c.http("aB-_9"), None);
    }

    #[test]
    fn the_alpn_certificate_names_the_identifier_and_carries_the_critical_acme_extension() {
        let c = Challenges::default();
        let digest = [7u8; 32];
        c.answer_alpn("203.0.113.10", &digest).unwrap();
        let key = c.alpn("10.113.0.203.in-addr.arpa").unwrap();
        let der = key.cert[0].as_ref();
        assert!(
            der.windows(6).any(|w| w == [0x87, 0x04, 203, 0, 113, 10]),
            "iPAddress SAN"
        );
        let ext = [
            0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x1f, 0x01, 0x01, 0xff, 0x04,
            0x22, 0x04, 0x20,
        ];
        let at = der
            .windows(ext.len())
            .position(|w| w == ext)
            .expect("acmeIdentifier");
        assert_eq!(&der[at + ext.len()..at + ext.len() + 32], &digest);
        c.answer_alpn("blog.example.com", &digest).unwrap();
        assert!(c.alpn("BLOG.example.com.").is_some());
        c.forget(&["blog.example.com".into(), "203.0.113.10".into()], &[]);
        assert!(c.alpn("blog.example.com").is_none());
        assert!(c.alpn("10.113.0.203.in-addr.arpa").is_none());
    }
}
