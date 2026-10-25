use std::path::PathBuf;
use std::time::Duration;

use burrow_syntax::cidr::Cidr;
use http::{HeaderName, HeaderValue};

pub const RESPONSE_HEADER_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteOptions {
    pub paths: Vec<String>,
    pub app_path: Option<String>,
    pub redirect_from: Vec<String>,
    pub redirects: Vec<Redirect>,
    pub maintenance: bool,
    pub target_tls: TargetTls,
    pub target_name: String,
    pub hsts: bool,
    pub compress: bool,
    pub max_body_size: Option<u64>,
    pub response_header_timeout: Duration,
    pub request_headers: Vec<HeaderRule>,
    pub response_headers: Vec<HeaderRule>,
    pub error_pages: Option<PathBuf>,
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
            hsts: false,
            compress: false,
            max_body_size: None,
            response_header_timeout: RESPONSE_HEADER_TIMEOUT,
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            error_pages: None,
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
