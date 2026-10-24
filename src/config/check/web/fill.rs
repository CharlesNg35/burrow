use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use crate::config::settings::{
    CertFile, LetsEncrypt, WebSettings, WebSite, WebSocketSettings, WebTls,
};
use burrow_config::{Directory, DirectoryName, ServerConfig, ServiceWeb, SiteTls, TlsMode};

const LETSENCRYPT_PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";
const LETSENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

pub(super) fn site(
    cfg: &ServerConfig,
    web: &ServiceWeb,
    hosts: Vec<String>,
    base_dir: &Path,
) -> WebSite {
    let https = cfg.web.as_ref().is_some_and(|w| w.https.is_some());
    let tls = match web.tls() {
        Some(SiteTls::Mode(TlsMode::Passthrough)) => WebTls::Passthrough,
        Some(SiteTls::Mode(TlsMode::Auto)) => WebTls::Auto,
        Some(SiteTls::Mode(TlsMode::Off)) => WebTls::Off,
        Some(SiteTls::Files(f)) => WebTls::Files {
            cert: base_dir.join(&f.cert),
            key: base_dir.join(&f.key),
        },
        None if https => WebTls::Auto,
        None => WebTls::Off,
    };
    let https_redirect = web
        .https_redirect()
        .or(cfg.web.as_ref().and_then(|w| w.https_redirect))
        .unwrap_or(tls != WebTls::Off);
    WebSite {
        hosts,
        tls,
        https_redirect,
    }
}

fn directory_url(d: Option<&Directory>) -> String {
    match d {
        None | Some(Directory::Named(DirectoryName::Production)) => LETSENCRYPT_PRODUCTION.into(),
        Some(Directory::Named(DirectoryName::Staging)) => LETSENCRYPT_STAGING.into(),
        Some(Directory::Url(u)) => u.trim().to_string(),
    }
}

pub(in crate::config::check) fn settings(
    cfg: &ServerConfig,
    ports: (Option<SocketAddr>, Option<SocketAddr>),
    sites: BTreeMap<String, WebSite>,
    websocket: Option<&WebSocketSettings>,
    base_dir: &Path,
) -> Option<WebSettings> {
    let (https, http) = ports;
    let web = cfg.web.as_ref()?;
    let secs = |v: Option<u64>, d: Duration| v.map_or(d, Duration::from_secs);
    let mut s = WebSettings::new(https?);
    s.http = http;
    s.sites = sites;
    s.tunnel = websocket.and_then(|w| w.host.clone());
    s.request_header_timeout = secs(web.request_header_timeout_secs, s.request_header_timeout);
    s.idle_timeout = secs(web.idle_timeout_secs, s.idle_timeout);
    s.certificates = web
        .certificates
        .iter()
        .map(|c| CertFile {
            hosts: c.hosts.clone(),
            cert: base_dir.join(&c.cert),
            key: base_dir.join(&c.key),
        })
        .collect();
    s.letsencrypt = web.letsencrypt.as_ref().map(|l| LetsEncrypt {
        email: l.email.trim().to_string(),
        directory: directory_url(l.directory.as_ref()),
        ca: l.ca.as_ref().map(|c| base_dir.join(c)),
    });
    s.file_digest = crate::config::settings::digest_of(&s.cert_files());
    Some(s)
}
