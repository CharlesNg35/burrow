use std::net::SocketAddr;

use crate::config::settings::ServerSettings;
pub use burrow_admin_proto::web::{HostLine, TunnelLine, WebSummary};

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
                kind: "passthrough".into(),
                certificate: "the app's own certificate".into(),
                service: service.clone(),
            })
        })
        .collect();
    hosts.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    debug_assert!(
        hosts.windows(2).all(|w| w[0].name != w[1].name),
        "validate lets each host name lead to one service"
    );
    Some(WebSummary {
        https: web.https.to_string(),
        addresses: addresses(web.https).into(),
        shared_with_tunnel: tunnel.is_some(),
        tunnel,
        hosts,
    })
}

pub fn text(s: &WebSummary) -> String {
    let mut out = s.lines().join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests;
