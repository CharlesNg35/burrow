pub mod acme;
pub mod client_auth;
pub mod files;
mod fixed;
mod select;
mod store;
pub mod x509;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use tokio_rustls::rustls::server::ResolvesServerCert;
use tokio_rustls::rustls::sign::CertifiedKey;

use crate::{CertSettings, CertSource};

pub type Applying<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub use fixed::Fixed;
pub use select::{TUNNEL_GROUP, cert_names};
pub use store::{Store, now_secs};

pub trait Certificates: ResolvesServerCert + Send + Sync + 'static {
    fn state(&self, name: &str) -> CertState;

    fn certificate(&self, name: &str) -> Option<Arc<CertifiedKey>>;

    fn alpn_challenge(&self, name: &str) -> Option<Arc<CertifiedKey>>;

    fn http_challenge(&self, token: &str) -> Option<String>;

    fn apply<'a>(&'a self, settings: &'a CertSettings, names: &'a [CertName]) -> Applying<'a>;

    fn status(&self) -> Vec<CertStatus>;

    fn account(&self) -> Option<String> {
        None
    }

    fn queue(&self) -> Vec<CertOrder> {
        Vec::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertOrder {
    pub service: String,
    pub names: Vec<String>,
    pub at: u64,
    pub running: bool,
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
    pub group: String,
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
    pub code: Option<&'static str>,
    pub explanation: Option<String>,
    pub failures: u64,
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
    Dns,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertPhase {
    Ok,
    Pending,
    Renewing,
    Failing { reason: String },
}
