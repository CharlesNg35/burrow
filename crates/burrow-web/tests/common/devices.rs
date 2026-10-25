use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rcgen::{
    BasicConstraints, CertificateParams, CertificateRevocationListParams, DistinguishedName,
    DnType, IsCa, Issuer, KeyIdMethod, KeyPair, KeyUsagePurpose, RevokedCertParams, SerialNumber,
};
use time::OffsetDateTime;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::client::{ClientSessionMemoryCache, ClientSessionStore, Resumption};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio_rustls::rustls::{self, ClientConfig, HandshakeKind, NamedGroup};

use super::{AnyCert, provider};

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

pub struct Device {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: Arc<KeyPair>,
}

pub fn device(ca: &DeviceCa, serial: u64, chain_extra: &[&DeviceCa]) -> Device {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, format!("device {serial}"));
    params.distinguished_name = dn;
    params.serial_number = Some(SerialNumber::from(serial));
    let cert = params
        .signed_by(&key, &Issuer::from_params(&ca.params, &ca.key))
        .unwrap();
    let mut chain = vec![CertificateDer::from(cert.der().to_vec())];
    for c in chain_extra {
        let der = rustls::pki_types::pem::PemObject::from_pem_slice(c.pem.as_bytes()).unwrap();
        chain.push(der);
    }
    Device {
        chain,
        key: Arc::new(key),
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

#[derive(Debug)]
pub struct NameBlind(ClientSessionMemoryCache);

fn one() -> ServerName<'static> {
    ServerName::try_from("any.invalid").unwrap()
}

impl ClientSessionStore for NameBlind {
    fn set_kx_hint(&self, _: ServerName<'static>, group: NamedGroup) {
        self.0.set_kx_hint(one(), group);
    }

    fn kx_hint(&self, _: &ServerName<'_>) -> Option<NamedGroup> {
        self.0.kx_hint(&one())
    }

    fn set_tls12_session(
        &self,
        _: ServerName<'static>,
        v: rustls::client::Tls12ClientSessionValue,
    ) {
        self.0.set_tls12_session(one(), v);
    }

    fn tls12_session(&self, _: &ServerName<'_>) -> Option<rustls::client::Tls12ClientSessionValue> {
        self.0.tls12_session(&one())
    }

    fn remove_tls12_session(&self, _: &ServerName<'static>) {
        self.0.remove_tls12_session(&one());
    }

    fn insert_tls13_ticket(
        &self,
        _: ServerName<'static>,
        v: rustls::client::Tls13ClientSessionValue,
    ) {
        self.0.insert_tls13_ticket(one(), v);
    }

    fn take_tls13_ticket(
        &self,
        _: &ServerName<'static>,
    ) -> Option<rustls::client::Tls13ClientSessionValue> {
        self.0.take_tls13_ticket(&one())
    }
}

pub fn name_blind() -> Arc<NameBlind> {
    Arc::new(NameBlind(ClientSessionMemoryCache::new(32)))
}

pub fn client(device: Option<&Device>, store: Option<Arc<NameBlind>>) -> TlsConnector {
    let b = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCert));
    let mut config = match device {
        Some(d) => {
            let key = PrivateKeyDer::try_from(d.key.serialize_der()).unwrap();
            b.with_client_auth_cert(d.chain.clone(), key).unwrap()
        }
        None => b.with_no_client_auth(),
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    if let Some(s) = store {
        config.resumption = Resumption::store(s);
    }
    TlsConnector::from(Arc::new(config))
}

pub type Shake = std::io::Result<TlsStream<TcpStream>>;

pub async fn shake(addr: SocketAddr, name: &str, c: &TlsConnector) -> Shake {
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::try_from(name.to_string()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), c.connect(name, tcp))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
}

pub async fn get(addr: SocketAddr, name: &str, host: &str, c: &TlsConnector) -> Option<String> {
    let mut t = shake(addr, name, c).await.ok()?;
    let req = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    let text = super::exchange(&mut t, req.as_bytes()).await;
    (!text.is_empty()).then_some(text)
}

pub async fn kind(addr: SocketAddr, name: &str, c: &TlsConnector) -> Option<HandshakeKind> {
    let mut t = shake(addr, name, c).await.ok()?;
    let k = t.get_ref().1.handshake_kind();
    let req = format!("GET / HTTP/1.1\r\nHost: {name}\r\nConnection: close\r\n\r\n");
    let text = super::exchange(&mut t, req.as_bytes()).await;
    text.starts_with("HTTP/1.1").then_some(k).flatten()
}
