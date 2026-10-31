use std::borrow::Cow;
use std::net::{IpAddr, SocketAddr};

use burrow_syntax::cidr::Cidr;
use http::HeaderMap;
use http::header::{self, HeaderName};

pub const FORWARDED: &str = "forwarded";
pub const FORWARDED_FOR: &str = "x-forwarded-for";
pub const FORWARDED_PROTO: &str = "x-forwarded-proto";
pub const FORWARDED_HOST: &str = "x-forwarded-host";
pub const FORWARDED_PORT: &str = "x-forwarded-port";
pub const FORWARDED_PREFIX: &str = "x-forwarded-prefix";
pub const FORWARDED_URI: &str = "x-forwarded-uri";
pub const FORWARDED_METHOD: &str = "x-forwarded-method";
pub const REAL_IP: &str = "x-real-ip";
pub const REQUEST_ID: &str = "x-request-id";

pub const MANAGED: [&str; 14] = [
    FORWARDED,
    FORWARDED_FOR,
    FORWARDED_PROTO,
    FORWARDED_HOST,
    FORWARDED_PORT,
    FORWARDED_PREFIX,
    "x-forwarded-server",
    FORWARDED_URI,
    FORWARDED_METHOD,
    "x-forwarded-ssl",
    "x-forwarded-scheme",
    "x-scheme",
    REAL_IP,
    REQUEST_ID,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub ip: IpAddr,
    pub trusted_peer: bool,
    pub https: bool,
    pub port: u16,
    pub forwarded_for: String,
}

pub fn is_trusted(trusted: &[Cidr], ip: IpAddr) -> bool {
    trusted.iter().any(|c| c.contains(ip))
}

fn values<'a>(h: &'a HeaderMap, name: &'static str) -> impl Iterator<Item = &'a str> {
    h.get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
}

pub fn entry_ip(entry: &str) -> Option<IpAddr> {
    if let Ok(ip) = entry.parse::<IpAddr>() {
        return Some(ip.to_canonical());
    }
    if let Ok(sa) = entry.parse::<SocketAddr>() {
        return Some(sa.ip().to_canonical());
    }
    let inner = entry.strip_prefix('[')?.strip_suffix(']')?;
    inner.parse::<IpAddr>().ok().map(|ip| ip.to_canonical())
}

pub fn client_ip(peer: IpAddr, h: &HeaderMap, trusted: &[Cidr]) -> IpAddr {
    let peer = peer.to_canonical();
    if !is_trusted(trusted, peer) {
        return peer;
    }
    let chain: Vec<&str> = values(h, FORWARDED_FOR).collect();
    let mut last = peer;
    for entry in chain.iter().rev() {
        let Some(ip) = entry_ip(entry) else {
            return last;
        };
        if !is_trusted(trusted, ip) {
            return ip;
        }
        last = ip;
    }
    last
}

fn trusted_proto(h: &HeaderMap) -> Option<bool> {
    let last = values(h, FORWARDED_PROTO).last()?;
    match last.to_ascii_lowercase().as_str() {
        "https" => Some(true),
        "http" => Some(false),
        _ => None,
    }
}

fn host_port(h: &HeaderMap) -> Option<u16> {
    let host = h.get(header::HOST)?.to_str().ok()?;
    let after = match host.rfind(']') {
        Some(i) => &host[i + 1..],
        None => host,
    };
    after.rsplit_once(':')?.1.parse().ok()
}

pub fn client(peer: SocketAddr, h: &HeaderMap, trusted: &[Cidr], tls: bool) -> Client {
    let trusted_peer = is_trusted(trusted, peer.ip());
    let ip = client_ip(peer.ip(), h, trusted);
    let https = if trusted_peer {
        trusted_proto(h).unwrap_or(tls)
    } else {
        tls
    };
    let given_port = trusted_peer
        .then(|| values(h, FORWARDED_PORT).last()?.parse::<u16>().ok())
        .flatten();
    let port = given_port
        .or_else(|| host_port(h))
        .unwrap_or(if https { 443 } else { 80 });
    let peer_ip = peer.ip().to_canonical().to_string();
    let chain: Vec<&str> = if trusted_peer {
        values(h, FORWARDED_FOR).filter(|v| !v.is_empty()).collect()
    } else {
        Vec::new()
    };
    let forwarded_for = if chain.is_empty() {
        peer_ip
    } else {
        format!("{}, {peer_ip}", chain.join(", "))
    };
    Client {
        ip,
        trusted_peer,
        https,
        port,
        forwarded_for,
    }
}

pub fn managed(name: &HeaderName) -> bool {
    MANAGED.contains(&name.as_str())
}

pub(crate) fn dashed(name: &HeaderName) -> Cow<'_, str> {
    let n = name.as_str();
    if n.contains('_') {
        Cow::Owned(n.replace('_', "-"))
    } else {
        Cow::Borrowed(n)
    }
}

fn underscored(name: &HeaderName) -> bool {
    matches!(dashed(name), Cow::Owned(n) if MANAGED.contains(&n.as_str()))
}

pub fn strip_incoming(h: &mut HeaderMap, trusted_peer: bool) {
    let names: Vec<HeaderName> = h
        .keys()
        .filter(|n| underscored(n) || (!trusted_peer && managed(n)))
        .cloned()
        .collect();
    for n in names {
        h.remove(n);
    }
}

#[cfg(test)]
mod tests;
