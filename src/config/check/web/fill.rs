use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use crate::config::settings::{
    CertFile, LetsEncrypt, WebSettings, WebSite, WebSocketSettings, WebTls,
};
use burrow_config::{Directory, DirectoryName, ServerConfig, ServiceWeb, SiteTls, TlsMode};

use crate::config::settings::{LETSENCRYPT_PRODUCTION, LETSENCRYPT_STAGING};

pub(super) fn site(
    cfg: &ServerConfig,
    web: &ServiceWeb,
    target: &burrow_config::Target,
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
    let challenge = match web {
        ServiceWeb::Site(s) => super::challenge::choice(s.challenge),
        _ => None,
    };
    let options = cfg.web.as_ref().map_or_else(Default::default, |top| {
        super::fill_site::options(top, web, target, base_dir)
    });
    WebSite {
        hosts,
        tls,
        https_redirect,
        challenge,
        options,
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
    if https.is_none() && http.is_none() {
        return None;
    }
    let mut s = WebSettings::on((https, http));
    s.sites = sites;
    s.tunnel = websocket.and_then(|w| w.host.clone());
    s.tunnel_challenge = super::challenge::choice(cfg.websocket.as_ref().and_then(|w| w.challenge));
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
        dns: super::fill_dns::entries(&l.dns, base_dir),
        email: l.email.trim().to_string(),
        directory: directory_url(l.directory.as_ref()),
        ca: l.ca.as_ref().map(|c| base_dir.join(c)),
        state_dir: None,
        profile: l
            .profile
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string),
    });
    super::fill_site::site_wide(web, &mut s, base_dir);
    s.file_digest = crate::config::settings::digest_of(&s.cert_files());
    Some(s)
}
