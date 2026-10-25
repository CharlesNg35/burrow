use std::path::PathBuf;
use std::time::Duration;

pub const RESPONSE_HEADER_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteOptions {
    pub paths: Vec<String>,
    pub app_path: Option<String>,
    pub redirect_from: Vec<String>,
    pub redirects: Vec<PathRedirect>,
    pub maintenance: bool,
    pub target_tls: TargetTls,
    pub target_name: String,
    pub target_http2: bool,
    pub hsts: bool,
    pub compress: bool,
    pub max_body_size: Option<u64>,
    pub response_header_timeout: Duration,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
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
            target_http2: false,
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
pub struct PathRedirect {
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
pub enum TargetTls {
    Off,
    PublicRoots,
    Ca(PathBuf),
    Unchecked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Common,
    Json,
}
