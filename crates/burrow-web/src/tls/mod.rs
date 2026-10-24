mod fixed;

use std::path::PathBuf;
use std::sync::Arc;

use tokio_rustls::rustls::server::ResolvesServerCert;
use tokio_rustls::rustls::sign::CertifiedKey;

use crate::{CertSettings, CertSource};

pub use fixed::Fixed;

pub trait Certificates: ResolvesServerCert + Send + Sync + 'static {
    fn state(&self, name: &str) -> CertState;

    fn alpn_challenge(&self, name: &str) -> Option<Arc<CertifiedKey>>;

    fn http_challenge(&self, token: &str) -> Option<String>;

    fn apply(&self, settings: &CertSettings, names: &[CertName]);

    fn status(&self) -> Vec<CertStatus>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertState {
    Ready,
    Pending,
    Failing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertName {
    pub name: String,
    pub source: CertSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertStatus {
    pub name: String,
    pub origin: CertOrigin,
    pub check: Option<CertCheck>,
    pub phase: CertPhase,
    pub expires: Option<u64>,
    pub next_renewal: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertOrigin {
    OwnFile { path: PathBuf },
    SharedFile { path: PathBuf },
    TunnelFile { path: PathBuf },
    LetsEncrypt { directory: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertCheck {
    Tls,
    Http,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertPhase {
    Ok,
    Pending,
    Renewing,
    Failing { reason: String },
}
