use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use crate::cidr::{Cidr, canonical};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetRule {
    pub net: Cidr,
    pub ports: Option<(u16, u16)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketRule {
    path: String,
    folder: bool,
}

impl SocketRule {
    pub fn matches(&self, path: &str) -> bool {
        if crate::unix::check_path(path).is_err() {
            return false;
        }
        if self.folder {
            path.strip_prefix(self.path.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .is_some_and(|name| !name.is_empty() && !name.contains('/'))
        } else {
            path == self.path
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn folder(&self) -> bool {
        self.folder
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    Net(NetRule),
    Socket(SocketRule),
}

impl Rule {
    pub fn matches(&self, target: SocketAddr) -> bool {
        match self {
            Rule::Net(r) => r.matches(target),
            Rule::Socket(_) => false,
        }
    }

    pub fn matches_socket(&self, path: &str) -> bool {
        match self {
            Rule::Socket(r) => r.matches(path),
            Rule::Net(_) => false,
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rule::Net(r) => r.fmt(f),
            Rule::Socket(r) if r.folder => write!(f, "unix:{}/*", r.path),
            Rule::Socket(r) => write!(f, "unix:{}", r.path),
        }
    }
}

impl NetRule {
    pub fn example(&self) -> SocketAddr {
        let port = self.ports.map_or(22, |(lo, _)| lo);
        let local = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
        let ip = match self.net.addr() {
            _ if self.net.contains(local) => local,
            IpAddr::V4(ip) if self.net.prefix() < 32 => IpAddr::V4((u32::from(ip) + 1).into()),
            IpAddr::V6(ip) if self.net.prefix() < 128 => IpAddr::V6((u128::from(ip) + 1).into()),
            ip => ip,
        };
        SocketAddr::new(ip, port)
    }

    pub fn matches(&self, target: SocketAddr) -> bool {
        self.net.contains(target.ip())
            && self
                .ports
                .is_none_or(|(lo, hi)| (lo..=hi).contains(&target.port()))
    }
}

impl fmt::Display for NetRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let net = match self.net.addr() {
            IpAddr::V6(_) if self.ports.is_some() => format!("[{}]", self.net),
            _ => self.net.to_string(),
        };
        match self.ports {
            None => write!(f, "{net}"),
            Some((lo, hi)) if lo == hi => write!(f, "{net}:{lo}"),
            Some((lo, hi)) => write!(f, "{net}:{lo}-{hi}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleError {
    Address(String),
    Prefix(String),
    Port(String),
    HostBits(String),
    Socket(String),
}

impl RuleError {
    pub fn describe(&self) -> String {
        match self {
            RuleError::Address(a) => format!("\"{a}\" is not an IP address or CIDR"),
            RuleError::Prefix(p) => format!("\"{p}\" is not a valid prefix length"),
            RuleError::Port(p) => format!("\"{p}\" is not a valid port or port range"),
            RuleError::HostBits(net) => format!("\"{net}\" has host bits set"),
            RuleError::Socket(why) => why.clone(),
        }
    }
}

impl FromStr for Rule {
    type Err = RuleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let Some(path) = s.strip_prefix("unix:") else {
            return s.parse().map(Rule::Net);
        };
        let (path, folder) = match path.strip_suffix("/*") {
            Some(dir) => (dir, true),
            None => (path, false),
        };
        let checked = if folder && path.trim_matches('/').is_empty() {
            Err(
                "allowing every socket on the machine (unix:/*) is too broad; name a folder"
                    .to_string(),
            )
        } else if path.ends_with('/') || path.contains("//") {
            Err(format!(
                "\"{s}\" has a stray \"/\"; write unix:/folder/* for every socket in a folder, or the full path of one socket"
            ))
        } else {
            crate::unix::check_path(path)
        };
        checked.map_err(RuleError::Socket)?;
        Ok(Rule::Socket(SocketRule {
            path: path.to_string(),
            folder,
        }))
    }
}

impl FromStr for NetRule {
    type Err = RuleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (net, ports) = split_ports(s);
        let (addr, prefix) = match net.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (net, None),
        };
        let raw: IpAddr = addr
            .parse()
            .map_err(|_| RuleError::Address(addr.to_string()))?;
        let ip = canonical(raw);
        let mapped = raw != ip;
        let cidr = match prefix {
            None => Cidr::host(ip),
            Some(p) => p
                .parse::<u8>()
                .ok()
                .and_then(|n| if mapped { n.checked_sub(96) } else { Some(n) })
                .and_then(|n| Cidr::new(ip, n))
                .ok_or_else(|| RuleError::Prefix(p.to_string()))?,
        };
        if cidr.has_host_bits() {
            return Err(RuleError::HostBits(net.to_string()));
        }
        let ports = ports.map(parse_ports).transpose()?;
        Ok(NetRule { net: cidr, ports })
    }
}

impl FromStr for Cidr {
    type Err = RuleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rule: NetRule = s.parse()?;
        match rule.ports {
            None => Ok(rule.net),
            Some(_) => Err(RuleError::Address(s.trim().to_string())),
        }
    }
}

pub fn parse_cidr_loose(s: &str) -> Result<Cidr, RuleError> {
    let s = s.trim();
    let (addr, prefix) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (s, None),
    };
    let raw: IpAddr = addr
        .parse()
        .map_err(|_| RuleError::Address(addr.to_string()))?;
    let ip = canonical(raw);
    let mapped = raw != ip;
    match prefix {
        None => Ok(Cidr::host(ip)),
        Some(p) => p
            .parse::<u8>()
            .ok()
            .and_then(|n| if mapped { n.checked_sub(96) } else { Some(n) })
            .and_then(|n| Cidr::new(ip, n))
            .ok_or_else(|| RuleError::Prefix(p.to_string())),
    }
}

fn split_ports(s: &str) -> (&str, Option<&str>) {
    if let Some(rest) = s.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((net, "")) => (net, None),
            Some((net, tail)) => (net, Some(tail.strip_prefix(':').unwrap_or(tail))),
            None => (s, None),
        };
    }
    match s.matches(':').count() {
        1 => {
            let (net, port) = s.split_once(':').unwrap_or((s, ""));
            (net, Some(port))
        }
        _ => (s, None),
    }
}

fn parse_ports(p: &str) -> Result<(u16, u16), RuleError> {
    let bad = || RuleError::Port(p.to_string());
    let (lo, hi) = match p.split_once('-') {
        Some((lo, hi)) => (lo, hi),
        None => (p, p),
    };
    let lo: u16 = lo.parse().map_err(|_| bad())?;
    let hi: u16 = hi.parse().map_err(|_| bad())?;
    if lo == 0 || lo > hi {
        return Err(bad());
    }
    Ok((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(s: &str) -> Rule {
        s.parse().unwrap()
    }

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn socket_rules_with_a_stray_slash_are_refused() {
        for bad in [
            "unix:/run/app/",
            "unix:/run/app/*/",
            "unix:/run/app//*",
            "unix:/run//x.sock",
        ] {
            assert!(bad.parse::<Rule>().is_err(), "{bad}");
        }
        for good in ["unix:/run/app/*", "unix:/run/app/x.sock"] {
            assert!(good.parse::<Rule>().is_ok(), "{good}");
        }
    }

    #[test]
    fn parses_forms() {
        assert_eq!(rule("127.0.0.0/8").to_string(), "127.0.0.0/8");
        assert_eq!(rule("10.0.0.5").to_string(), "10.0.0.5/32");
        assert_eq!(rule("192.168.1.0/24:22").to_string(), "192.168.1.0/24:22");
        assert_eq!(
            rule("10.0.0.0/8:8000-8100").to_string(),
            "10.0.0.0/8:8000-8100"
        );
        assert_eq!(rule("::1").to_string(), "::1/128");
        assert_eq!(rule("fd00::/8").to_string(), "fd00::/8");
        assert_eq!(rule("[fd00::/8]:443").to_string(), "[fd00::/8]:443");
        assert_eq!(rule("[::1]:22").to_string(), "[::1/128]:22");
        assert_eq!(rule("::ffff:10.0.0.5").to_string(), "10.0.0.5/32");
        assert_eq!(
            rule("[::ffff:127.0.0.1/128]:3000").to_string(),
            "127.0.0.1/32:3000"
        );
        assert_eq!(rule("::ffff:10.0.0.0/104").to_string(), "10.0.0.0/8");
        assert!("::ffff:10.0.0.0/64".parse::<Rule>().is_err());
        assert_eq!(rule("[::1]").to_string(), "::1/128");
        for empty in ["[::1]:", "[::ffff:10.0.0.1]:", "10.0.0.1:", "[::1]x"] {
            assert!(empty.parse::<Rule>().is_err(), "{empty}");
        }
    }

    #[test]
    fn rejects_bad() {
        assert!(matches!(
            "10.0.0.0/33".parse::<Rule>(),
            Err(RuleError::Prefix(_))
        ));
        assert!(matches!(
            "10.0.0/8".parse::<Rule>(),
            Err(RuleError::Address(_))
        ));
        assert!(matches!(
            "10.0.0.1/8".parse::<Rule>(),
            Err(RuleError::HostBits(_))
        ));
        assert!(matches!(
            "10.0.0.0/8:0".parse::<Rule>(),
            Err(RuleError::Port(_))
        ));
        assert!(matches!(
            "10.0.0.0/8:90-80".parse::<Rule>(),
            Err(RuleError::Port(_))
        ));
        assert!(matches!(
            "10.0.0.0/8:http".parse::<Rule>(),
            Err(RuleError::Port(_))
        ));
        assert!(matches!(
            "example.com".parse::<Rule>(),
            Err(RuleError::Address(_))
        ));
    }

    #[test]
    fn cidrs_parse() {
        assert!("10.0.0.0/8:22".parse::<Cidr>().is_err());
        assert!("10.0.0.1/8".parse::<Cidr>().is_err());
        assert!("home".parse::<Cidr>().is_err());
        let loose = parse_cidr_loose("192.168.1.10/24").unwrap();
        assert!(loose.has_host_bits());
        assert_eq!(loose.network().to_string(), "192.168.1.0/24");
    }

    #[test]
    fn socket_rule_errors() {
        for bad in [
            "unix:relative",
            "unix:/*",
            "unix://*",
            "unix:/a/../b",
            "unix:",
        ] {
            assert!(
                matches!(bad.parse::<Rule>(), Err(RuleError::Socket(_))),
                "{bad}"
            );
        }
        assert_eq!(
            rule("unix:/run/postgresql/*").to_string(),
            "unix:/run/postgresql/*"
        );
    }

    #[test]
    fn zero_prefix_matches_family() {
        let r = rule("0.0.0.0/0");
        assert!(r.matches(sa("8.8.8.8:1")));
        assert!(!r.matches(sa("[2001::1]:1")));
    }
}
