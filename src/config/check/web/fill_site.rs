use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::config::settings::{
    ForwardAuth, LogFormat, NameRedirect, OidcSettings, PathRedirect, RESPONSE_HEADER_TIMEOUT,
    SiteOidc, SiteOptions, TargetTls, WebClientAuth, WebSettings,
};
use crate::policy::allowlist::Cidr;
use burrow_config::{
    AccessLogFormat, LoginPick, OidcPick, RedirectTarget, ServiceWeb, Target,
    TargetTls as TargetTlsKey, WebConfig, WebSite,
};
use burrow_syntax::web::{SingleLabel, host_name};

pub(super) fn options(
    top: &WebConfig,
    web: &ServiceWeb,
    target: &Target,
    base_dir: &Path,
) -> SiteOptions {
    match web {
        ServiceWeb::Site(s) => site_options(top, s, target, base_dir),
        _ => site_options(top, &bare_site(), target, base_dir),
    }
}

fn bare_site() -> WebSite {
    WebSite {
        hosts: Vec::new(),
        paths: Vec::new(),
        tls: None,
        https_redirect: None,
        challenge: None,
        app_path: None,
        redirect_from: Vec::new(),
        redirects: BTreeMap::new(),
        maintenance: false,
        target_tls: None,
        target_http2: false,
        basic_auth: BTreeMap::new(),
        forward_auth: None,
        oidc: None,
        allow_users: Vec::new(),
        rate_limit: None,
        client_auth: None,
        hsts: None,
        compress: None,
        max_body_size: None,
        response_header_timeout_secs: None,
        request_headers: BTreeMap::new(),
        response_headers: BTreeMap::new(),
        error_pages: None,
    }
}

fn site_options(top: &WebConfig, s: &WebSite, target: &Target, base_dir: &Path) -> SiteOptions {
    let WebSite {
        hosts: _,
        tls: _,
        https_redirect: _,
        challenge: _,
        paths,
        app_path,
        redirect_from,
        redirects,
        maintenance,
        target_tls,
        target_http2,
        basic_auth,
        forward_auth,
        oidc,
        allow_users,
        rate_limit,
        client_auth,
        hsts,
        compress,
        max_body_size,
        response_header_timeout_secs,
        request_headers,
        response_headers,
        error_pages,
    } = s;
    let mut basic_auth: Vec<(String, String)> = basic_auth
        .iter()
        .map(|(u, h)| (u.trim().to_string(), h.trim().to_string()))
        .collect();
    basic_auth.sort();
    let target_tls = match target_tls {
        None | Some(TargetTlsKey::Checked(false)) => TargetTls::Off,
        Some(TargetTlsKey::Checked(true)) => TargetTls::PublicRoots,
        Some(TargetTlsKey::Ca(c)) => {
            TargetTls::Ca(base_dir.join(c.ca.as_deref().unwrap_or_default().trim()))
        }
        Some(TargetTlsKey::Unchecked(u)) if u.insecure_skip_verify => TargetTls::Unchecked,
        Some(TargetTlsKey::Unchecked(_)) => TargetTls::PublicRoots,
    };
    SiteOptions {
        paths: paths.iter().map(|p| path(p)).collect(),
        app_path: app_path.as_deref().map(path),
        redirect_from: redirect_from
            .iter()
            .filter_map(|h| host_name(h, SingleLabel::Allowed).ok())
            .collect(),
        redirects: redirects
            .iter()
            .map(|(from, t)| PathRedirect {
                from: path(from),
                to: t.to().trim().to_string(),
                permanent: t.permanent(),
            })
            .collect(),
        maintenance: *maintenance,
        target_name: match target_tls {
            TargetTls::Off => String::new(),
            _ => target_name(target),
        },
        target_tls,
        target_http2: *target_http2,
        basic_auth,
        forward_auth: match forward_auth.as_ref().map(|f| f.pick(&top.forward_auth)) {
            Some(LoginPick::Found(_, f)) => Some(forward(f, top)),
            _ => None,
        },
        oidc: match oidc.as_ref().map(|l| l.pick(&top.oidc)) {
            Some(OidcPick::Found(names)) => Some(SiteOidc {
                providers: names.into_iter().map(str::to_string).collect(),
                allow_users: allow_users.iter().map(|u| u.trim().to_string()).collect(),
            }),
            _ => None,
        },
        rate_limit: rate_limit.or(top.rate_limit).filter(|r| *r > 0),
        client_auth: client_auth.as_ref().map(|c| WebClientAuth {
            ca: base_dir.join(c.ca.as_deref().unwrap_or_default().trim()),
            crl: c.crl.as_ref().map(|p| base_dir.join(p.trim())),
        }),
        hsts: hsts.or(top.hsts).unwrap_or(false),
        compress: compress.or(top.compress).unwrap_or(false),
        max_body_size: max_body_size
            .as_deref()
            .or(top.max_body_size.as_deref())
            .and_then(size),
        response_header_timeout: secs(
            response_header_timeout_secs.or(top.response_header_timeout_secs),
        ),
        request_headers: headers(&top.request_headers, request_headers),
        response_headers: headers(&top.response_headers, response_headers),
        error_pages: error_pages
            .as_ref()
            .or(top.error_pages.as_ref())
            .map(|p| base_dir.join(p.trim())),
    }
}

pub(super) fn site_wide(top: &WebConfig, s: &mut WebSettings, base_dir: &Path) {
    s.redirect_names = top
        .redirect_names
        .iter()
        .filter_map(|(name, t)| name_redirect(name, t))
        .collect();
    s.trusted_proxies = cidrs(&top.trusted_proxies);
    s.proxy_protocol_from = cidrs(&top.proxy_protocol_from);
    s.access_log = top.access_log.as_ref().map(|a| {
        let format = match a.format() {
            AccessLogFormat::Common => LogFormat::Common,
            AccessLogFormat::Json => LogFormat::Json,
        };
        (base_dir.join(a.file().trim()), format)
    });
    s.request_id = top.request_id.unwrap_or(false);
    s.oidc = top
        .oidc
        .iter()
        .map(|(name, o)| OidcSettings {
            name: name.clone(),
            issuer: o.issuer.trim().to_string(),
            client_id: o.client_id.trim().to_string(),
            client_secret: o
                .client_secret
                .as_deref()
                .and_then(|v| super::fill_dns::secret(v, base_dir)),
            secret_file: o
                .client_secret
                .as_deref()
                .and_then(|v| v.trim().strip_prefix(super::fill_dns::FILE_PREFIX))
                .map(|p| base_dir.join(p.trim())),
        })
        .collect();
}

fn name_redirect(name: &str, t: &RedirectTarget) -> Option<NameRedirect> {
    let name = name.trim();
    let (host, rest) = match name.find('/') {
        Some(i) => (&name[..i], &name[i..]),
        None => (name, "/"),
    };
    Some(NameRedirect {
        host: host_name(host, SingleLabel::Allowed).ok()?,
        path: path(rest),
        to: t.to().trim().to_string(),
        permanent: t.permanent(),
    })
}

fn forward(f: &burrow_config::ForwardAuthConfig, top: &WebConfig) -> ForwardAuth {
    let names = |f: &burrow_config::ForwardAuthConfig| {
        f.copy_headers
            .iter()
            .map(|h| h.trim().to_string())
            .collect::<Vec<_>>()
    };
    let mut strip: Vec<String> = top.forward_auth.values().flat_map(names).collect();
    strip.sort_by_key(|h| h.to_ascii_lowercase());
    strip.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    ForwardAuth {
        service: f.service.trim().to_string(),
        path: path(&f.path),
        copy_headers: names(f),
        strip_headers: strip,
    }
}

fn path(p: &str) -> String {
    burrow_syntax::url_path::route_path(p).unwrap_or_else(|_| p.trim().to_string())
}

fn target_name(target: &Target) -> String {
    target.host().unwrap_or_else(|| "localhost".to_string())
}

fn size(text: &str) -> Option<u64> {
    burrow_syntax::size::parse_size(text).ok()
}

fn secs(v: Option<u64>) -> Duration {
    v.map_or(RESPONSE_HEADER_TIMEOUT, Duration::from_secs)
}

fn headers(
    top: &BTreeMap<String, String>,
    own: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut out: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (name, value) in top.iter().chain(own) {
        let name = name.trim();
        out.insert(
            name.to_ascii_lowercase(),
            (name.to_string(), value.trim().to_string()),
        );
    }
    out.into_values().collect()
}

fn cidrs(list: &[String]) -> Vec<Cidr> {
    list.iter().filter_map(|c| c.trim().parse().ok()).collect()
}
