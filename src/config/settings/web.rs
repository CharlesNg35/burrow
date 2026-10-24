use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

pub const REQUEST_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSettings {
    pub https: SocketAddr,
    pub http: Option<SocketAddr>,
    pub sites: BTreeMap<String, WebSite>,
    pub tunnel: Option<String>,
    pub request_header_timeout: Duration,
    pub idle_timeout: Duration,
    pub certificates: Vec<CertFile>,
    pub letsencrypt: Option<LetsEncrypt>,
    pub file_digest: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSite {
    pub hosts: Vec<String>,
    pub tls: WebTls,
    pub https_redirect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebTls {
    Passthrough,
    Auto,
    Files { cert: PathBuf, key: PathBuf },
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertFile {
    pub hosts: Vec<String>,
    pub cert: PathBuf,
    pub key: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetsEncrypt {
    pub email: String,
    pub directory: String,
    pub ca: Option<PathBuf>,
}

impl WebSettings {
    pub fn new(https: SocketAddr) -> WebSettings {
        WebSettings {
            https,
            http: None,
            sites: BTreeMap::new(),
            tunnel: None,
            request_header_timeout: REQUEST_HEADER_TIMEOUT,
            idle_timeout: IDLE_TIMEOUT,
            certificates: Vec::new(),
            letsencrypt: None,
            file_digest: 0,
        }
    }

    pub fn cert_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for site in self.sites.values() {
            if let WebTls::Files { cert, key } = &site.tls {
                out.extend([cert.clone(), key.clone()]);
            }
        }
        for c in &self.certificates {
            out.extend([c.cert.clone(), c.key.clone()]);
        }
        out.extend(self.letsencrypt.as_ref().and_then(|l| l.ca.clone()));
        out
    }

    pub fn site_of(&self, service: &str) -> Option<&WebSite> {
        self.sites.get(service)
    }
}

impl WebSite {
    pub fn passthrough(hosts: Vec<String>) -> WebSite {
        WebSite {
            hosts,
            tls: WebTls::Passthrough,
            https_redirect: true,
        }
    }
}

pub fn digest_of(files: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for f in files {
        f.hash(&mut h);
        std::fs::read(f).ok().hash(&mut h);
    }
    h.finish()
}
