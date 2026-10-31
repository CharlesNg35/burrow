use burrow_web::{
    AccessLog, BasicUser, ClientAuth, DnsEntry, DnsProvider, ForwardAuth, Forwarding, HeaderRule,
    LogFormat, NameRedirect, OidcProvider, ProxySettings, Redirect, Secret, SiteLogin, SiteOptions,
    TargetTls,
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
        target_http2: o.target_http2,
        basic_auth: o
            .basic_auth
            .iter()
            .map(|(user, hash)| BasicUser {
                user: user.clone(),
                hash: hash.clone(),
            })
            .collect(),
        forward_auth: o.forward_auth.as_ref().map(|f| ForwardAuth {
            service: f.service.clone(),
            path: f.path.clone(),
            copy_headers: header_names(&f.copy_headers),
            strip_headers: header_names(&f.strip_headers),
        }),
        oidc: o.oidc.as_ref().map(|l| SiteLogin {
            providers: l.providers.clone(),
            allow_users: l.allow_users.clone(),
        }),
        rate_limit: o.rate_limit.and_then(std::num::NonZeroU32::new),
        client_auth: o.client_auth.as_ref().map(|c| ClientAuth {
            ca: c.ca.clone(),
            crl: c.crl.clone(),
        }),
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
    p.http2 = web.http2;
    p.oidc = web
        .oidc
        .iter()
        .map(|o| OidcProvider {
            name: o.name.clone(),
            issuer: o.issuer.clone(),
            client_id: o.client_id.clone(),
            client_secret: o
                .client_secret
                .as_ref()
                .map(|s| Secret::new(s.expose().to_string())),
        })
        .collect();
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
                    algorithm,
                } => DnsProvider::Rfc2136 {
                    server: server.clone(),
                    key_name: key_name.clone(),
                    secret: secret(s),
                    algorithm: algorithm
                        .as_deref()
                        .and_then(burrow_web::TsigAlgorithm::parse)
                        .unwrap_or_default(),
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

fn header_names(names: &[String]) -> Vec<HeaderName> {
    names
        .iter()
        .filter_map(|h| HeaderName::from_bytes(h.as_bytes()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_web::TsigAlgorithm;

    fn algorithm_of(written: Option<&str>) -> TsigAlgorithm {
        let entry = settings::DnsSettings {
            domains: vec!["example.org".into()],
            provider: settings::DnsProvider::Rfc2136 {
                server: "10.0.0.53:53".into(),
                key_name: "k".into(),
                secret: settings::SecretText::new("c2VjcmV0".into()),
                algorithm: written.map(str::to_string),
            },
            wait: std::time::Duration::from_secs(120),
            resolvers: Vec::new(),
            wildcards: Vec::new(),
        };
        match &dns_entries(&[entry])[0].provider {
            DnsProvider::Rfc2136 { algorithm, .. } => *algorithm,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_tsig_algorithm_reaches_burrow_web_and_is_hmac_sha256_when_not_written() {
        assert_eq!(algorithm_of(Some("HMAC-SHA512")), TsigAlgorithm::HmacSha512);
        assert_eq!(
            algorithm_of(Some("hmac-sha384.")),
            TsigAlgorithm::HmacSha384
        );
        assert_eq!(algorithm_of(None), TsigAlgorithm::HmacSha256);
    }
}
