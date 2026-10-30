use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::config::settings::{
    ForwardAuth, LogFormat, NameRedirect, PathRedirect, RESPONSE_HEADER_TIMEOUT, SiteOptions,
    TargetTls, WebClientAuth, WebSettings,
};
use crate::policy::allowlist::Cidr;
use burrow_config::{
    AccessLogFormat, LoginPick, RedirectTarget, ServiceWeb, Target, TargetTls as TargetTlsKey,
    WebConfig,
};
use burrow_syntax::web::{SingleLabel, host_name};

pub(super) fn options(
    top: &WebConfig,
    web: &ServiceWeb,
    target: &Target,
    base_dir: &Path,
) -> SiteOptions {
    let mut o = SiteOptions {
        hsts: top.hsts.unwrap_or(false),
        compress: top.compress.unwrap_or(false),
        max_body_size: top.max_body_size.as_deref().and_then(size),
        response_header_timeout: secs(top.response_header_timeout_secs),
        request_headers: headers(&top.request_headers, &BTreeMap::new()),
        response_headers: headers(&top.response_headers, &BTreeMap::new()),
        error_pages: top.error_pages.as_ref().map(|p| base_dir.join(p.trim())),
        rate_limit: top.rate_limit.filter(|r| *r > 0),
        ..SiteOptions::default()
    };
    let ServiceWeb::Site(s) = web else {
        return o;
    };
    o.paths = s.paths.iter().map(|p| path(p)).collect();
    o.app_path = s.app_path.as_deref().map(path);
    o.redirect_from = s
        .redirect_from
        .iter()
        .filter_map(|h| host_name(h, SingleLabel::Allowed).ok())
        .collect();
    o.redirects = s
        .redirects
        .iter()
        .map(|(from, t)| PathRedirect {
            from: path(from),
            to: t.to().trim().to_string(),
            permanent: t.permanent(),
        })
        .collect();
    o.maintenance = s.maintenance;
    o.target_http2 = s.target_http2;
    o.basic_auth = s
        .basic_auth
        .iter()
        .map(|(u, h)| (u.trim().to_string(), h.trim().to_string()))
        .collect();
    o.basic_auth.sort();
    o.forward_auth = match s.forward_auth.as_ref().map(|f| f.pick(&top.forward_auth)) {
        Some(LoginPick::Found(_, f)) => Some(forward(f, top)),
        _ => None,
    };
    o.rate_limit = s.rate_limit.or(top.rate_limit).filter(|r| *r > 0);
    o.client_auth = s.client_auth.as_ref().map(|c| WebClientAuth {
        ca: base_dir.join(c.ca.as_deref().unwrap_or_default().trim()),
        crl: c.crl.as_ref().map(|p| base_dir.join(p.trim())),
    });
    o.target_tls = match &s.target_tls {
        None | Some(TargetTlsKey::Checked(false)) => TargetTls::Off,
        Some(TargetTlsKey::Checked(true)) => TargetTls::PublicRoots,
        Some(TargetTlsKey::Ca(c)) => {
            TargetTls::Ca(base_dir.join(c.ca.as_deref().unwrap_or_default().trim()))
        }
        Some(TargetTlsKey::Unchecked(u)) if u.insecure_skip_verify => TargetTls::Unchecked,
        Some(TargetTlsKey::Unchecked(_)) => TargetTls::PublicRoots,
    };
    if o.target_tls != TargetTls::Off {
        o.target_name = target_name(target);
    }
    o.hsts = s.hsts.unwrap_or(o.hsts);
    o.compress = s.compress.unwrap_or(o.compress);
    if let Some(m) = s.max_body_size.as_deref() {
        o.max_body_size = size(m);
    }
    if let Some(t) = s.response_header_timeout_secs {
        o.response_header_timeout = Duration::from_secs(t);
    }
    o.request_headers = headers(&top.request_headers, &s.request_headers);
    o.response_headers = headers(&top.response_headers, &s.response_headers);
    if let Some(p) = &s.error_pages {
        o.error_pages = Some(base_dir.join(p.trim()));
    }
    o
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
    let hp = target.host_port();
    let hp = hp
        .strip_prefix("unix:")
        .map_or(hp.as_str(), |_| "localhost");
    if let Some(inner) = hp.strip_prefix('[').and_then(|r| r.split_once(']')) {
        return inner.0.to_string();
    }
    match hp.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.parse::<u16>().is_ok() => {
            host.to_string()
        }
        _ => hp.to_string(),
    }
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
