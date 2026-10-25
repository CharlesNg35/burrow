use std::net::SocketAddr;

use crate::config::settings::{
    LETSENCRYPT_PRODUCTION, LETSENCRYPT_STAGING, LetsEncrypt, ServerSettings, Shares, WebSettings,
    WebSite, WebTls,
};
pub use burrow_admin_proto::web::{HostLine, PLAIN, TunnelLine, WebSummary};
use burrow_syntax::web::{Challenge, Check, CheckPorts, checks_for, entry_for, is_ip};

fn addresses(a: SocketAddr) -> &'static str {
    match (a.ip().is_unspecified(), a.is_ipv6()) {
        (true, true) => "IPv4 and IPv6",
        (true, false) => "IPv4 only",
        (false, _) => "this address only",
    }
}

fn shown(a: Option<SocketAddr>) -> (String, String) {
    a.map_or_else(Default::default, |a| (a.to_string(), addresses(a).into()))
}

fn account(l: &LetsEncrypt) -> String {
    let directory = match l.directory.as_str() {
        LETSENCRYPT_PRODUCTION => "Let's Encrypt (production)".to_string(),
        LETSENCRYPT_STAGING => "Let's Encrypt (staging)".to_string(),
        other => format!("the ACME server {other}"),
    };
    let mut out = format!("{directory} as {}", l.email);
    if let Some(p) = &l.profile {
        out.push_str(&format!(", profile {p}"));
    }
    if let Some(ca) = &l.ca {
        out.push_str(&format!(", trusting {}", ca.display()));
    }
    if let Some(d) = &l.state_dir {
        out.push_str(&format!("; kept in {}", d.display()));
    }
    out
}

fn automatic(web: &WebSettings, name: &str, choice: Option<Challenge>) -> (String, String) {
    let entries = web
        .certificates
        .iter()
        .map(|c| c.hosts.as_slice())
        .enumerate();
    if let Ok(Some(i)) = entry_for(entries, name) {
        return (
            format!(
                "web.certificates[{i}] ({})",
                web.certificates[i].cert.display()
            ),
            String::new(),
        );
    }
    if web.letsencrypt.is_none() {
        return ("no certificate".into(), String::new());
    }
    let ports = CheckPorts {
        https: web.https.map(|a| a.port()),
        http: web.http.map(|a| a.port()),
    };
    let check = match checks_for(name, ports, false, choice).as_deref() {
        Ok([Check::Tls, ..]) => "TLS check",
        Ok([Check::Http, ..]) => "HTTP check",
        Ok([Check::Dns, ..]) => "DNS check",
        _ => "no check possible",
    };
    let source = if is_ip(name) {
        "Let's Encrypt short-lived (6 days)"
    } else {
        "Let's Encrypt"
    };
    (source.into(), check.into())
}

fn host(web: &WebSettings, service: &str, site: &WebSite, name: &str) -> HostLine {
    let port = web.http.map_or(String::new(), |a| a.to_string());
    let (kind, certificate, check) = match &site.tls {
        WebTls::Passthrough => (
            "passthrough",
            "passthrough, the app's own certificate".to_string(),
            String::new(),
        ),
        WebTls::Off => {
            let why = if web.https.is_some() {
                " (tls: off)"
            } else {
                ""
            };
            (
                PLAIN,
                format!("plain HTTP on {port} only, unencrypted{why}"),
                String::new(),
            )
        }
        WebTls::Files { cert, .. } => (
            "HTTPS",
            format!("own file {}", cert.display()),
            String::new(),
        ),
        WebTls::Auto => {
            let (source, check) = automatic(web, name, site.challenge);
            ("HTTPS", source, check)
        }
    };
    let both = kind == "HTTPS" && !site.https_redirect && web.http.is_some();
    HostLine {
        name: name.to_string(),
        kind: kind.into(),
        certificate,
        check,
        note: if both {
            "http:// served too (https_redirect: false)".into()
        } else {
            String::new()
        },
        service: service.to_string(),
    }
}

fn http_note(web: &WebSettings) -> String {
    let mut parts = Vec::new();
    let redirects = web
        .sites
        .values()
        .any(|s| matches!(s.tls, WebTls::Auto | WebTls::Files { .. }) && s.https_redirect);
    if redirects {
        parts.push("HTTPS hosts redirect to https:// (308)");
    }
    if web.letsencrypt.is_some() {
        parts.push("Let's Encrypt HTTP checks answered");
    }
    parts.join("; ")
}

fn tunnel_line(s: &ServerSettings, web: &WebSettings) -> Option<TunnelLine> {
    let ws = s.websocket.as_ref()?;
    let url = ws.shared_url()?;
    let certificate = match (&ws.tls, ws.host.as_deref()) {
        _ if ws.shares == Shares::Http => "plain ws://, burrow-encrypted".to_string(),
        (Some(t), _) => format!("websocket.tls {}", t.cert_path.display()),
        (None, Some(name)) => {
            let (source, check) = automatic(web, name, web.tunnel_challenge);
            if check.is_empty() {
                source
            } else {
                format!("{source}, {check}")
            }
        }
        (None, None) => String::new(),
    };
    Some(TunnelLine { url, certificate })
}

pub fn summary(s: &ServerSettings) -> Option<WebSummary> {
    let web = s.web.as_ref().filter(|_| cfg!(feature = "web"))?;
    let tunnel = tunnel_line(s, web);
    let shares = s.websocket.as_ref().map_or(Shares::Nothing, |w| w.shares);
    let mut hosts: Vec<HostLine> = web
        .sites
        .iter()
        .flat_map(|(service, site)| site.hosts.iter().map(move |h| host(web, service, site, h)))
        .collect();
    hosts.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    debug_assert!(
        hosts.windows(2).all(|w| w[0].name != w[1].name),
        "validate lets each host name lead to one service"
    );
    let (https, addresses) = shown(web.https);
    let (http, http_addresses) = shown(web.http);
    Some(WebSummary {
        https,
        addresses,
        http,
        http_addresses,
        shared_with_tunnel: tunnel.is_some() && shares == Shares::Https,
        http_shared_with_tunnel: tunnel.is_some() && shares == Shares::Http,
        certificates: web.letsencrypt.as_ref().map(account).unwrap_or_default(),
        tunnel,
        hosts,
        http_note: http_note(web),
    })
}

pub fn text(s: &WebSummary) -> String {
    let mut out = s.lines().join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests;
