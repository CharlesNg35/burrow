use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use crate::config::settings::{DNS_WAIT, DnsProvider, DnsSettings, SecretText};
use burrow_config::DnsEntry;
use burrow_syntax::web::{SingleLabel, host_name};

pub(super) const FILE_PREFIX: &str = "file:";
const DNS_PORT: u16 = 53;

pub(super) fn entries(list: &[DnsEntry], base_dir: &Path) -> Vec<DnsSettings> {
    list.iter().filter_map(|e| entry(e, base_dir)).collect()
}

fn entry(e: &DnsEntry, base_dir: &Path) -> Option<DnsSettings> {
    Some(DnsSettings {
        domains: names(&e.domains),
        provider: provider(e, base_dir)?,
        wait: e.wait_secs.map_or(DNS_WAIT, Duration::from_secs),
        resolvers: e.resolvers.iter().filter_map(|r| resolver(r)).collect(),
        wildcards: names(&e.wildcards),
    })
}

fn provider(e: &DnsEntry, base_dir: &Path) -> Option<DnsProvider> {
    let secret = |v: &str| secret(v, base_dir);
    match (&e.cloudflare, &e.rfc2136, &e.acme_dns, &e.command) {
        (Some(c), None, None, None) => Some(DnsProvider::Cloudflare {
            token: secret(&c.token)?,
        }),
        (None, Some(r), None, None) => Some(DnsProvider::Rfc2136 {
            server: r.server.trim().to_string(),
            key_name: r.key_name.trim().to_string(),
            secret: secret(&r.secret)?,
            algorithm: r.algorithm.as_deref().map(|a| a.trim().to_string()),
        }),
        (None, None, Some(a), None) => Some(DnsProvider::AcmeDns {
            api: a.api.trim().to_string(),
            username: secret(&a.username)?,
            password: secret(&a.password)?,
            subdomain: a.subdomain.trim().to_string(),
        }),
        (None, None, None, Some(c)) => Some(DnsProvider::Command {
            path: base_dir.join(c.trim()),
        }),
        _ => None,
    }
}

pub(super) fn secret(value: &str, base_dir: &Path) -> Option<SecretText> {
    let text = match value.trim().strip_prefix(FILE_PREFIX) {
        Some(path) => {
            let read = std::fs::read_to_string(base_dir.join(path.trim())).ok()?;
            zeroize::Zeroizing::new(read).trim().to_string()
        }
        None => value.trim().to_string(),
    };
    (!text.is_empty()).then(|| SecretText::new(text))
}

fn names(list: &[String]) -> Vec<String> {
    list.iter()
        .filter_map(|n| host_name(n, SingleLabel::Allowed).ok())
        .collect()
}

pub(super) fn resolver(text: &str) -> Option<SocketAddr> {
    let t = text.trim();
    t.parse::<SocketAddr>().ok().or_else(|| {
        t.parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, DNS_PORT))
    })
}
