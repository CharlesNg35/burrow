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

    pub fn is_host(&self) -> bool {
        self.prefix == if self.addr.is_ipv4() { 32 } else { 128 }
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

    pub fn covers(&self, other: &Cidr) -> bool {
        self.prefix <= other.prefix
            && self.addr.is_ipv4() == other.addr.is_ipv4()
            && self.contains(other.addr)
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

    pub fn network(&self) -> Cidr {
        match self.addr {
            IpAddr::V4(a) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                Cidr {
                    addr: IpAddr::V4((u32::from(a) & mask).into()),
                    prefix: self.prefix,
                }
            }
            IpAddr::V6(a) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                Cidr {
                    addr: IpAddr::V6((u128::from(a) & mask).into()),
                    prefix: self.prefix,
                }
            }
        }
    }

    pub fn last(&self) -> IpAddr {
        match self.addr {
            IpAddr::V4(a) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                IpAddr::V4((u32::from(a) | !mask).into())
            }
            IpAddr::V6(a) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                IpAddr::V6((u128::from(a) | !mask).into())
            }
        }
    }

    pub fn size_text(&self) -> String {
        let bits = if self.addr.is_ipv4() { 32 } else { 128 } - u32::from(self.prefix);
        match bits {
            0 => "1 address".into(),
            b if b < 64 => format!("{} addresses", 1u64 << b),
            b => format!("2^{b} addresses"),
        }
    }

    pub fn is_everyone(&self) -> bool {
        self.prefix == 0
    }
}

pub fn canonical(ip: IpAddr) -> IpAddr {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Cidr {
        let (a, p) = s.split_once('/').unwrap();
        Cidr::new(a.parse().unwrap(), p.parse().unwrap()).unwrap()
    }

    #[test]
    fn ranges_and_sizes() {
        let n = c("192.168.1.10/24");
        assert!(n.has_host_bits());
        assert_eq!(n.network().to_string(), "192.168.1.0/24");
        assert_eq!(n.last().to_string(), "192.168.1.255");
        assert_eq!(n.size_text(), "256 addresses");
        assert_eq!(c("10.0.0.1/32").size_text(), "1 address");
        assert!(c("10.0.0.0/8").covers(&c("10.1.0.0/16")));
        assert!(!c("10.1.0.0/16").covers(&c("10.0.0.0/8")));
        assert!(c("0.0.0.0/0").is_everyone());
        assert_eq!(c("::/0").size_text(), "2^128 addresses");
    }
}
