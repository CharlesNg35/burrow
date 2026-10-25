use burrow_web::{
    AccessLog, DnsEntry, DnsProvider, Forwarding, HeaderRule, LogFormat, NameRedirect,
    ProxySettings, Redirect, Secret, SiteOptions, TargetTls,
};
use http::{HeaderName, HeaderValue};

use crate::config::settings::{self, WebSettings};

pub(super) fn site_options(o: &settings::SiteOptions) -> SiteOptions {
    SiteOptions {
        paths: o.paths.clone(),
        app_path: o.app_path.clone(),
        redirect_from: o.redirect_from.clone(),
        redirects: o
            .redirects
            .iter()
            .map(|r| Redirect {
                from: r.from.clone(),
                to: r.to.clone(),
                permanent: r.permanent,
            })
            .collect(),
        maintenance: o.maintenance,
        target_tls: match &o.target_tls {
            settings::TargetTls::Off => TargetTls::Off,
            settings::TargetTls::PublicRoots => TargetTls::PublicRoots,
            settings::TargetTls::Ca(ca) => TargetTls::Ca(ca.clone()),
            settings::TargetTls::Unchecked => TargetTls::Unchecked,
        },
        target_name: o.target_name.clone(),
        hsts: o.hsts,
        compress: o.compress,
        max_body_size: o.max_body_size,
        response_header_timeout: o.response_header_timeout,
        request_headers: rules(&o.request_headers),
        response_headers: rules(&o.response_headers),
        error_pages: o.error_pages.clone(),
    }
}

pub(super) fn site_wide(web: &WebSettings, p: &mut ProxySettings) {
    p.redirect_names = web
        .redirect_names
        .iter()
        .map(|r| NameRedirect {
            host: r.host.clone(),
            path: r.path.clone(),
            to: r.to.clone(),
            permanent: r.permanent,
        })
        .collect();
    p.forwarding = Forwarding {
        trusted_proxies: web.trusted_proxies.clone(),
        proxy_protocol_from: web.proxy_protocol_from.clone(),
    };
    p.access_log = web.access_log.as_ref().map(|(file, format)| AccessLog {
        file: file.clone(),
        format: match format {
            settings::LogFormat::Common => LogFormat::Common,
            settings::LogFormat::Json => LogFormat::Json,
        },
    });
    p.request_id = web.request_id;
}

fn rules(list: &[(String, String)]) -> Vec<HeaderRule> {
    list.iter()
        .filter_map(|(name, value)| {
            Some(HeaderRule {
                name: HeaderName::from_bytes(name.as_bytes()).ok()?,
                value: if value.is_empty() {
                    None
                } else {
                    Some(HeaderValue::from_str(value).ok()?)
                },
            })
        })
        .collect()
}

pub(super) fn dns_entries(list: &[settings::DnsSettings]) -> Vec<DnsEntry> {
    let secret = |s: &settings::SecretText| Secret::new(s.expose().to_string());
    list.iter()
        .map(|e| DnsEntry {
            domains: e.domains.clone(),
            provider: match &e.provider {
                settings::DnsProvider::Cloudflare { token } => DnsProvider::Cloudflare {
                    token: secret(token),
                },
                settings::DnsProvider::Rfc2136 {
                    server,
                    key_name,
                    secret: s,
                } => DnsProvider::Rfc2136 {
                    server: server.clone(),
                    key_name: key_name.clone(),
                    secret: secret(s),
                },
                settings::DnsProvider::AcmeDns {
                    api,
                    username,
                    password,
                    subdomain,
                } => DnsProvider::AcmeDns {
                    api: api.clone(),
                    username: username.expose().to_string(),
                    password: secret(password),
                    subdomain: subdomain.clone(),
                },
                settings::DnsProvider::Command { path } => {
                    DnsProvider::Command { path: path.clone() }
                }
            },
            wait: e.wait,
            resolvers: e.resolvers.clone(),
            wildcards: e.wildcards.clone(),
        })
        .collect()
}
