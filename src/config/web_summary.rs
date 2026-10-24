use std::net::SocketAddr;

use serde::Serialize;

use crate::config::settings::ServerSettings;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebSummary {
    pub https: String,
    pub addresses: &'static str,
    pub shared_with_tunnel: bool,
    pub tunnel: Option<TunnelLine>,
    pub hosts: Vec<HostLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TunnelLine {
    pub url: String,
    pub certificate: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostLine {
    pub name: String,
    pub kind: &'static str,
    pub certificate: &'static str,
    pub service: String,
}

fn addresses(a: SocketAddr) -> &'static str {
    match (a.ip().is_unspecified(), a.is_ipv6()) {
        (true, true) => "IPv4 and IPv6",
        (true, false) => "IPv4 only",
        (false, _) => "this address only",
    }
}

pub fn summary(s: &ServerSettings) -> Option<WebSummary> {
    let web = s.web.as_ref()?;
    let tunnel = s.websocket.as_ref().and_then(|w| {
        let url = w.shared_url()?;
        let certificate = w.tls.as_ref().map_or_else(String::new, |t| {
            format!("websocket.tls {}", t.cert_path.display())
        });
        Some(TunnelLine { url, certificate })
    });
    let mut hosts: Vec<HostLine> = web
        .sites
        .iter()
        .flat_map(|(service, site)| {
            site.hosts.iter().map(move |h| HostLine {
                name: h.clone(),
                kind: "passthrough",
                certificate: "the app's own certificate",
                service: service.clone(),
            })
        })
        .collect();
    hosts.sort_by(|a, b| a.name.cmp(&b.name));
    Some(WebSummary {
        https: web.https.to_string(),
        addresses: addresses(web.https),
        shared_with_tunnel: tunnel.is_some(),
        tunnel,
        hosts,
    })
}

impl WebSummary {
    pub fn lines(&self) -> Vec<String> {
        let shared = if self.shared_with_tunnel {
            "; shared with the tunnel"
        } else {
            ""
        };
        let mut out = vec![format!(
            "web: https {} ({}{shared})",
            self.https, self.addresses
        )];
        if let Some(t) = &self.tunnel {
            out.push(format!("tunnel: {} ({})", t.url, t.certificate));
        }
        for h in &self.hosts {
            out.push(format!(
                "{}: {}, {} -> service {}",
                h.name, h.kind, h.certificate, h.service
            ));
        }
        if self.hosts.is_empty() && self.tunnel.is_none() {
            out.push("no names yet: every connection is closed".into());
        }
        out
    }

    pub fn text(&self) -> String {
        let mut s = self.lines().join("\n");
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests;
