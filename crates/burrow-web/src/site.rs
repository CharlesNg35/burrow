use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::Duration;

use burrow_syntax::cidr::Cidr;
use http::{HeaderName, HeaderValue};

pub use burrow_syntax::defaults::RESPONSE_HEADER_TIMEOUT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteOptions {
    pub paths: Vec<String>,
    pub app_path: Option<String>,
    pub redirect_from: Vec<String>,
    pub redirects: Vec<Redirect>,
    pub maintenance: bool,
    pub target_tls: TargetTls,
    pub target_name: String,
    pub target_http2: bool,
    pub basic_auth: Vec<BasicUser>,
    pub forward_auth: Option<ForwardAuth>,
    pub oidc: Option<SiteLogin>,
    pub rate_limit: Option<NonZeroU32>,
    pub client_auth: Option<ClientAuth>,
    pub hsts: bool,
    pub compress: bool,
    pub max_body_size: Option<u64>,
    pub response_header_timeout: Duration,
    pub request_headers: Vec<HeaderRule>,
    pub response_headers: Vec<HeaderRule>,
    pub error_pages: Option<PathBuf>,
    pub app_key: u64,
}

impl Default for SiteOptions {
    fn default() -> SiteOptions {
        SiteOptions {
            paths: Vec::new(),
            app_path: None,
            redirect_from: Vec::new(),
            redirects: Vec::new(),
            maintenance: false,
            target_tls: TargetTls::Off,
            target_name: String::new(),
            target_http2: false,
            basic_auth: Vec::new(),
            forward_auth: None,
            oidc: None,
            rate_limit: None,
            client_auth: None,
            hsts: false,
            compress: false,
            max_body_size: None,
            response_header_timeout: RESPONSE_HEADER_TIMEOUT,
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            error_pages: None,
            app_key: 0,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct BasicUser {
    pub user: String,
    pub hash: String,
}

impl std::fmt::Debug for BasicUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BasicUser")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardAuth {
    pub service: String,
    pub path: String,
    pub copy_headers: Vec<HeaderName>,
    pub strip_headers: Vec<HeaderName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteLogin {
    pub providers: Vec<String>,
    pub allow_users: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcProvider {
    pub name: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<crate::Secret>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientAuth {
    pub ca: PathBuf,
    pub crl: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub from: String,
    pub to: String,
    pub permanent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRedirect {
    pub host: String,
    pub path: String,
    pub to: String,
    pub permanent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderRule {
    pub name: HeaderName,
    pub value: Option<HeaderValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetTls {
    Off,
    PublicRoots,
    Ca(PathBuf),
    Unchecked,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forwarding {
    pub trusted_proxies: Vec<Cidr>,
    pub proxy_protocol_from: Vec<Cidr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLog {
    pub file: PathBuf,
    pub format: LogFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Common,
    Json,
}
