use std::fmt;
use std::net::IpAddr;

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

    pub fn addr(&self) -> IpAddr {
        self.addr
    }

    pub fn prefix(&self) -> u8 {
        self.prefix
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Span {
    v6: bool,
    start: u128,
    end: u128,
}

fn span(c: &Cidr) -> Span {
    let (addr, prefix) = match c.addr {
        IpAddr::V6(a) if c.prefix >= 96 => match a.to_ipv4_mapped() {
            Some(v4) => (IpAddr::V4(v4), c.prefix - 96),
            None => (c.addr, c.prefix),
        },
        a => (a, c.prefix),
    };
    let (v6, bits, value) = match addr {
        IpAddr::V4(a) => (false, 32, u128::from(u32::from(a))),
        IpAddr::V6(a) => (true, 128, u128::from(a)),
    };
    let host = u128::MAX
        .checked_shr(128 - bits + u32::from(prefix))
        .unwrap_or(0);
    Span {
        v6,
        start: value & !host,
        end: (value & !host) | host,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spans(Vec<Span>);

impl Spans {
    pub fn of(list: &[Cidr]) -> Spans {
        let mut all: Vec<Span> = list.iter().map(span).collect();
        all.sort();
        let mut merged: Vec<Span> = Vec::new();
        for s in all {
            match merged.last_mut() {
                Some(m) if m.v6 == s.v6 && s.start <= m.end.saturating_add(1) => {
                    m.end = m.end.max(s.end);
                }
                _ => merged.push(s),
            }
        }
        Spans(merged)
    }

    pub fn covers(&self, c: &Cidr) -> bool {
        let s = span(c);
        self.0
            .iter()
            .any(|m| m.v6 == s.v6 && m.start <= s.start && s.end <= m.end)
    }

    fn covers_everything(&self) -> bool {
        EVERYWHERE.iter().all(|c| self.covers(c))
    }
}

pub const EVERYWHERE: [Cidr; 2] = [
    Cidr {
        addr: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        prefix: 0,
    },
    Cidr {
        addr: IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        prefix: 0,
    },
];

pub fn within(new: &[Cidr], old: &[Cidr]) -> bool {
    if old.is_empty() {
        return true;
    }
    let spans = Spans::of(old);
    if new.is_empty() {
        return spans.covers_everything();
    }
    new.iter().all(|c| spans.covers(c))
}

impl fmt::Display for Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Cidr {
        let (a, p) = s.split_once('/').unwrap();
        Cidr::new(a.parse().unwrap(), p.parse().unwrap()).unwrap()
    }

    fn parsed(s: &str) -> Cidr {
        s.parse().unwrap()
    }

    #[test]
    fn host_bits() {
        assert!(c("192.168.1.10/24").has_host_bits());
        assert!(!c("192.168.1.0/24").has_host_bits());
        assert!(!c("10.0.0.1/32").has_host_bits());
    }

    fn list(s: &[&str]) -> Vec<Cidr> {
        s.iter().copied().map(parsed).collect()
    }

    fn inside(new: &[&str], old: &[&str]) -> bool {
        within(&list(new), &list(old))
    }

    #[test]
    fn a_range_is_within_a_wider_one_of_its_family() {
        assert!(inside(&["10.1.0.0/16"], &["10.0.0.0/8"]));
        assert!(inside(&["10.0.0.0/8"], &["10.0.0.0/8"]));
        assert!(!inside(&["10.0.0.0/8"], &["10.1.0.0/16"]));
        assert!(!inside(&["11.0.0.0/16"], &["10.0.0.0/8"]));
        assert!(inside(&["10.9.8.7"], &["10.0.0.0/8"]));
        assert!(!inside(&["10.0.0.0/8"], &["10.9.8.7"]));
        assert!(inside(&["10.9.8.7/32"], &["10.9.8.7"]));
        assert!(inside(&["2001:db8:1::/48"], &["2001:db8::/32"]));
        assert!(inside(&["2001:db8::7"], &["2001:db8::/32"]));
        assert!(!inside(&["2001:db8::/32"], &["2001:db8:1::/48"]));
        assert!(inside(
            &["10.1.0.0/16", "2001:db8::7"],
            &["10.0.0.0/8", "2001:db8::/32"]
        ));
        assert!(!inside(&["10.1.0.0/16", "192.0.2.1"], &["10.0.0.0/8"]));
    }

    #[test]
    fn coverage_is_by_the_union_of_the_old_ranges() {
        assert!(inside(&["10.0.0.0/24"], &["10.0.0.0/25", "10.0.0.128/25"]));
        assert!(inside(&["10.0.0.0/23"], &["10.0.1.0/24", "10.0.0.0/24"]));
        assert!(inside(&["10.0.0.64/26"], &["10.0.0.0/25", "10.0.0.100"]));
        assert!(!inside(&["10.0.0.0/24"], &["10.0.0.0/25", "10.0.0.192/26"]));
        assert!(inside(&["0.0.0.0/0"], &["0.0.0.0/1", "128.0.0.0/1"]));
        assert!(inside(&["::/0"], &["::/1", "8000::/1"]));
        assert!(inside(&["10.0.0.0/24", "10.0.0.5"], &["10.0.0.0/24"]));
    }

    #[test]
    fn families_never_mix_and_mapped_addresses_are_ipv4() {
        assert!(!inside(&["10.0.0.0/8"], &["::/0"]));
        assert!(!inside(&["2001:db8::/32"], &["0.0.0.0/0"]));
        assert!(!inside(&["0.0.0.0/0"], &["::/0"]));
        assert!(inside(&["::ffff:10.1.2.3"], &["10.0.0.0/8"]));
        assert!(inside(&["::ffff:10.1.0.0/112"], &["10.0.0.0/8"]));
        assert!(!inside(&["::ffff:10.1.2.3"], &["::/0"]));
        assert!(inside(&["10.1.2.3"], &["::ffff:10.0.0.0/104"]));
        assert!(!inside(&["11.0.0.0/8"], &["::ffff:10.0.0.0/104"]));
        let raw = Cidr::new("::ffff:10.0.0.0".parse().unwrap(), 104).unwrap();
        assert!(within(&[parsed("10.200.0.0/16")], &[raw]));
        assert!(within(&[raw], &[parsed("10.0.0.0/8")]));
        assert!(!within(&[raw], &[parsed("::/0")]));
    }

    #[test]
    fn an_empty_list_means_everywhere() {
        assert!(inside(&["10.0.0.0/8"], &[]));
        assert!(inside(&[], &[]));
        assert!(inside(&["0.0.0.0/0", "::/0"], &[]));
        assert!(!inside(&[], &["10.0.0.0/8"]));
        assert!(!inside(&[], &["0.0.0.0/0"]));
        assert!(!inside(&[], &["::/0"]));
        assert!(inside(&[], &["0.0.0.0/0", "::/0"]));
        assert!(inside(&[], &["::/0", "0.0.0.0/1", "128.0.0.0/1"]));
        assert!(inside(
            &["203.0.113.9", "2001:db8::/64"],
            &["0.0.0.0/0", "::/0"]
        ));
        assert!(Spans::of(&EVERYWHERE).covers_everything());
        assert!(!Spans::of(&[]).covers_everything());
    }
}
