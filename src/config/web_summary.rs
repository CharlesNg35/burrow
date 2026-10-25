use std::net::SocketAddr;

use crate::config::settings::DnsProvider;
use crate::config::settings::{
    LETSENCRYPT_PRODUCTION, LETSENCRYPT_STAGING, LetsEncrypt, ServerSettings, Shares, WebSettings,
    WebSite, WebTls, Written,
};
pub use burrow_admin_proto::web::{HostLine, PLAIN, TunnelLine, WebSummary};
use burrow_syntax::web::{
    Challenge, Check, CheckPorts, checks_for, dns_entry_for, entry_for, is_ip,
};

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

fn account(l: &LetsEncrypt, written: &Written) -> String {
    let directory = match l.directory.as_str() {
        LETSENCRYPT_PRODUCTION => "Let's Encrypt (production)".to_string(),
        LETSENCRYPT_STAGING => "Let's Encrypt (staging)".to_string(),
        other => format!("the ACME server {}", written.shown(other)),
    };
    let mut out = format!("{directory} as {}", written.shown(&l.email));
    if let Some(p) = &l.profile {
        out.push_str(&format!(", profile {}", written.shown(p)));
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
    if let Some(w) = shared_wildcard(web, name) {
        return (format!("Let's Encrypt shared wildcard {w}"), String::new());
    }
    let ports = CheckPorts {
        https: web.https.map(|a| a.port()),
        http: web.http.map(|a| a.port()),
    };
    let dns = dns_for(web, name);
    let check = match checks_for(name, ports, dns.is_some(), choice).as_deref() {
        Ok([Check::Tls, ..]) => "TLS check".to_string(),
        Ok([Check::Http, ..]) => "HTTP check".to_string(),
        Ok([Check::Dns, ..]) => {
            let (provider, domain) = dns.unwrap_or_default();
            let why = if choice == Some(Challenge::Dns) {
                " (challenge: dns)"
            } else {
                ""
            };
            format!("DNS check through {provider} ({domain}){why}")
        }
        _ => "no check possible".to_string(),
    };
    let source = if is_ip(name) {
        "Let's Encrypt short-lived (6 days)"
    } else {
        "Let's Encrypt"
    };
    (source.into(), check)
}

pub(super) fn provider_label(p: &DnsProvider) -> &'static str {
    match p {
        DnsProvider::Cloudflare { .. } => "Cloudflare",
        DnsProvider::Rfc2136 { .. } => "RFC 2136",
        DnsProvider::AcmeDns { .. } => "acme-dns",
        DnsProvider::Command { .. } => "the command",
    }
}

fn shared_wildcard(web: &WebSettings, name: &str) -> Option<String> {
    web.letsencrypt
        .as_ref()?
        .dns
        .iter()
        .flat_map(|e| e.wildcards.iter())
        .find(|w| {
            w.as_str() == name
                || w.strip_prefix("*.") == Some(name)
                || burrow_syntax::web::covers(w, name)
        })
        .cloned()
}

fn dns_for(web: &WebSettings, name: &str) -> Option<(String, String)> {
    let dns = &web.letsencrypt.as_ref()?.dns;
    let i = dns_entry_for(
        dns.iter()
            .enumerate()
            .map(|(i, e)| (i, e.domains.as_slice())),
        name,
    )?;
    let bare = name.strip_prefix("*.").unwrap_or(name);
    let domain = dns[i]
        .domains
        .iter()
        .filter(|d| bare == d.as_str() || bare.ends_with(&format!(".{d}")))
        .max_by_key(|d| d.len())
        .cloned()
        .unwrap_or_default();
    Some((provider_label(&dns[i].provider).to_string(), domain))
}

fn dns_line(l: &LetsEncrypt) -> String {
    l.dns
        .iter()
        .map(|e| {
            let found = if e.resolvers.is_empty() {
                "the system resolvers".to_string()
            } else {
                e.resolvers
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!(
                "DNS checks through {} for {}, nameservers found via {found}",
                provider_label(&e.provider),
                e.domains.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn wildcard_lines(web: &WebSettings) -> Vec<String> {
    let Some(l) = web.letsencrypt.as_ref() else {
        return Vec::new();
    };
    l.dns
        .iter()
        .flat_map(|e| e.wildcards.iter())
        .map(|w| {
            let parent = w.strip_prefix("*.").unwrap_or(w);
            let users: Vec<&str> = web
                .sites
                .iter()
                .filter(|(_, s)| matches!(s.tls, WebTls::Auto))
                .filter(|(_, s)| {
                    s.hosts
                        .iter()
                        .any(|h| h == w || h == parent || burrow_syntax::web::covers(w, h))
                })
                .map(|(n, _)| n.as_str())
                .collect();
            let used = if users.is_empty() {
                "used by nothing yet".to_string()
            } else {
                format!("used by {}", users.join(", "))
            };
            format!("wildcard {w} (+ {parent}): Let's Encrypt, DNS check; {used}")
        })
        .collect()
}

fn front(web: &WebSettings) -> String {
    let list = |c: &[crate::policy::allowlist::Cidr]| {
        c.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut parts = Vec::new();
    if !web.proxy_protocol_from.is_empty() {
        parts.push(format!(
            "PROXY headers required from {}",
            list(&web.proxy_protocol_from)
        ));
    }
    if !web.trusted_proxies.is_empty() {
        parts.push(format!(
            "X-Forwarded-For believed from {}",
            list(&web.trusted_proxies)
        ));
    }
    parts.join("; ")
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
        routes: Vec::new(),
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
    let hosts = routes::hosts(web);
    let (https, addresses) = shown(web.https);
    let (http, http_addresses) = shown(web.http);
    Some(WebSummary {
        https,
        addresses,
        http,
        http_addresses,
        shared_with_tunnel: tunnel.is_some() && shares == Shares::Https,
        http_shared_with_tunnel: tunnel.is_some() && shares == Shares::Http,
        certificates: web
            .letsencrypt
            .as_ref()
            .map(|l| match dns_line(l) {
                d if d.is_empty() => account(l, &web.written),
                d => format!("{}; {d}", account(l, &web.written)),
            })
            .unwrap_or_default(),
        tunnel,
        hosts,
        http_note: http_note(web),
        account: String::new(),
        front: front(web),
        protocols: match (web.https.is_some(), web.http2) {
            (false, _) => String::new(),
            (true, true) => "HTTP/2 and HTTP/1.1".into(),
            (true, false) => "HTTP/1.1 only".into(),
        },
        wildcards: wildcard_lines(web),
    })
}

#[cfg(feature = "web")]
pub fn account_url(s: &ServerSettings) -> String {
    use burrow_web::tls::acme::account::{Saved, saved};
    let Some(le) = s.web.as_ref().and_then(|w| w.letsencrypt.as_ref()) else {
        return String::new();
    };
    let Some(state) = le.state_dir.as_deref() else {
        return String::new();
    };
    let ca = match le.directory.as_str() {
        LETSENCRYPT_PRODUCTION | LETSENCRYPT_STAGING => "letsencrypt.org",
        _ => "the CA's name",
    };
    match saved(state, &le.directory) {
        Saved::Account(url) => format!(
            "{url}; a CAA record can let only this account get certificates: issue \"{ca}; accounturi={url}\""
        ),
        Saved::NotYet => "created on the first start; burrow status and validate then show its URL for a CAA accounturi record".into(),
        Saved::Unreadable(e) => format!(
            "not shown, this user cannot read {} ({e}); burrow status shows it",
            state.display()
        ),
    }
}

pub fn text(s: &WebSummary) -> String {
    let mut out = s.lines().join("\n");
    out.push('\n');
    out
}

mod routes;

#[cfg(test)]
mod tests;
