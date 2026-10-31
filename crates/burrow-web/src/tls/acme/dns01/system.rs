use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use super::wire::DNS_PORT;

pub const PUBLIC: [IpAddr; 3] = [
    IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
    IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolvers {
    pub addrs: Vec<SocketAddr>,
    pub system: bool,
}

impl Resolvers {
    pub fn describe(&self) -> String {
        if self.system {
            return "the system resolvers".into();
        }
        let list: Vec<String> = self.addrs.iter().map(|a| a.ip().to_string()).collect();
        format!("{} (no system resolvers found)", list.join(", "))
    }
}

fn usable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_unspecified(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            !v6.is_unspecified() && first & 0xffc0 != 0xfe80 && first & 0xffc0 != 0xfec0
        }
    }
}

pub fn pick(found: Vec<IpAddr>) -> Resolvers {
    let mut addrs: Vec<SocketAddr> = Vec::new();
    for ip in found.into_iter().filter(usable) {
        let a = SocketAddr::new(ip, DNS_PORT);
        if !addrs.contains(&a) {
            addrs.push(a);
        }
    }
    if addrs.is_empty() {
        return Resolvers {
            addrs: PUBLIC
                .iter()
                .map(|ip| SocketAddr::new(*ip, DNS_PORT))
                .collect(),
            system: false,
        };
    }
    Resolvers {
        addrs,
        system: true,
    }
}

pub fn from_resolv_conf(text: &[u8]) -> Vec<IpAddr> {
    let (config, _) = resolv_conf::Config::parse_with_errors(text);
    config.nameservers.into_iter().map(IpAddr::from).collect()
}

#[cfg(not(windows))]
pub fn system() -> Resolvers {
    pick(
        std::fs::read("/etc/resolv.conf")
            .map(|t| from_resolv_conf(&t))
            .unwrap_or_default(),
    )
}

#[cfg(windows)]
pub fn system() -> Resolvers {
    let found = ipconfig::get_adapters()
        .map(|list| {
            list.iter()
                .filter(|a| a.oper_status() == ipconfig::OperStatus::IfOperStatusUp)
                .flat_map(|a| a.dns_servers().to_vec())
                .collect()
        })
        .unwrap_or_default();
    pick(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolv_conf_nameservers_are_used_else_public_ones_named_in_the_summary() {
        let text = b"# generated\nnameserver 127.0.0.53\nnameserver 2001:db8::1\nnameserver fe80::1%eth0\noptions edns0\nsearch lan\n";
        let found = from_resolv_conf(text);
        let r = pick(found);
        assert_eq!(
            r.addrs,
            vec![
                "127.0.0.53:53".parse().unwrap(),
                "[2001:db8::1]:53".parse().unwrap()
            ]
        );
        assert_eq!(r.describe(), "the system resolvers");
        let none = pick(vec!["fec0:0:0:ffff::1".parse().unwrap()]);
        assert!(!none.system);
        assert_eq!(
            none.describe(),
            "1.1.1.1, 8.8.8.8, 9.9.9.9 (no system resolvers found)"
        );
        assert!(pick(from_resolv_conf(b"garbage line\n")).addrs.len() == 3);
    }
}
