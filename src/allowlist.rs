use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn new(addr: IpAddr, prefix: u8) -> Option<Self> {
        let max = if addr.is_ipv4() { 32 } else { 128 };
        (prefix <= max).then_some(Cidr { addr, prefix })
    }

    pub fn host(addr: IpAddr) -> Self {
        let prefix = if addr.is_ipv4() { 32 } else { 128 };
        Cidr { addr, prefix }
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = canonical(ip);
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }

    pub fn has_host_bits(&self) -> bool {
        match self.addr {
            IpAddr::V4(a) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(a) & !mask != 0
            }
            IpAddr::V6(a) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(a) & !mask != 0
            }
        }
    }
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

impl fmt::Display for Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub net: Cidr,
    pub ports: Option<(u16, u16)>,
}

impl Rule {
    pub fn matches(&self, target: SocketAddr) -> bool {
        self.net.contains(target.ip())
            && self
                .ports
                .is_none_or(|(lo, hi)| (lo..=hi).contains(&target.port()))
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let net = match self.net.addr {
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
}

impl RuleError {
    pub fn describe(&self) -> String {
        match self {
            RuleError::Address(a) => format!("\"{a}\" is not an IP address or CIDR"),
            RuleError::Prefix(p) => format!("\"{p}\" is not a valid prefix length"),
            RuleError::Port(p) => format!("\"{p}\" is not a valid port or port range"),
            RuleError::HostBits(net) => format!("\"{net}\" has host bits set"),
        }
    }
}

impl FromStr for Rule {
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
        Ok(Rule { net: cidr, ports })
    }
}

impl FromStr for Cidr {
    type Err = RuleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rule: Rule = s.parse()?;
        match rule.ports {
            None => Ok(rule.net),
            Some(_) => Err(RuleError::Address(s.trim().to_string())),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Sources {
    pub allow: Vec<Cidr>,
    pub deny: Vec<Cidr>,
}

impl Sources {
    pub fn permits(&self, ip: IpAddr) -> bool {
        (self.allow.is_empty() || self.allow.iter().any(|c| c.contains(ip)))
            && !self.deny.iter().any(|c| c.contains(ip))
    }

    pub fn is_open(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }
}

fn split_ports(s: &str) -> (&str, Option<&str>) {
    if let Some(rest) = s.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((net, tail)) => (
                net,
                tail.strip_prefix(':')
                    .or(Some(tail))
                    .filter(|t| !t.is_empty()),
            ),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowList {
    All,
    Rules(Vec<Rule>),
}

impl AllowList {
    pub fn permits(&self, target: SocketAddr) -> bool {
        match self {
            AllowList::All => true,
            AllowList::Rules(rules) => rules.iter().any(|r| r.matches(target)),
        }
    }

    pub fn range_suggestion(ip: IpAddr, first: u16, last: u16) -> String {
        match canonical(ip) {
            IpAddr::V4(ip) => format!("{ip}/32:{first}-{last}"),
            IpAddr::V6(ip) => format!("[{ip}/128]:{first}-{last}"),
        }
    }

    pub fn suggestion(target: SocketAddr) -> String {
        match canonical(target.ip()) {
            IpAddr::V4(ip) => format!("{ip}/32:{}", target.port()),
            IpAddr::V6(ip) => format!("[{ip}/128]:{}", target.port()),
        }
    }
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
    fn matching() {
        let list = AllowList::Rules(vec![
            rule("127.0.0.0/8"),
            rule("192.168.1.0/24:22"),
            rule("fd00::/8"),
        ]);
        assert!(list.permits(sa("127.0.0.1:3000")));
        assert!(list.permits(sa("192.168.1.50:22")));
        assert!(!list.permits(sa("192.168.1.50:23")));
        assert!(!list.permits(sa("192.168.2.1:22")));
        assert!(list.permits(sa("[fd12::1]:80")));
        assert!(list.permits(sa("[::ffff:127.0.0.1]:80")));
        assert!(!list.permits(sa("[::1]:80")));
        assert!(!AllowList::Rules(vec![]).permits(sa("127.0.0.1:1")));
        assert!(AllowList::All.permits(sa("8.8.8.8:53")));
    }

    #[test]
    fn source_filters() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let cidr = |s: &str| s.parse::<Cidr>().unwrap();
        assert!(Sources::default().permits(ip("8.8.8.8")));
        let office = Sources {
            allow: vec![cidr("203.0.113.0/24"), cidr("2001:db8::/32")],
            deny: vec![cidr("203.0.113.66")],
        };
        assert!(office.permits(ip("203.0.113.7")));
        assert!(office.permits(ip("::ffff:203.0.113.7")));
        assert!(office.permits(ip("2001:db8::1")));
        assert!(!office.permits(ip("203.0.113.66")));
        assert!(!office.permits(ip("198.51.100.1")));
        let blocklist = Sources {
            allow: vec![],
            deny: vec![cidr("45.9.0.0/16")],
        };
        assert!(blocklist.permits(ip("1.1.1.1")));
        assert!(!blocklist.permits(ip("45.9.1.2")));
        assert!("10.0.0.0/8:22".parse::<Cidr>().is_err());
        assert!("10.0.0.1/8".parse::<Cidr>().is_err());
        assert!("home".parse::<Cidr>().is_err());
    }

    #[test]
    fn zero_prefix_matches_family() {
        let r = rule("0.0.0.0/0");
        assert!(r.matches(sa("8.8.8.8:1")));
        assert!(!r.matches(sa("[2001::1]:1")));
    }
}
