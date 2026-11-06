use std::net::IpAddr;

use crate::address::is_localhost;
use crate::cidr::Cidr;

pub const PERMITTED_NAMES: [&str; 5] = ["localhost", "local", "internal", "home.arpa", "lan"];
pub const PERMITTED_NETS: [(&str, u8); 9] = [
    ("10.0.0.0", 8),
    ("172.16.0.0", 12),
    ("192.168.0.0", 16),
    ("127.0.0.0", 8),
    ("169.254.0.0", 16),
    ("100.64.0.0", 10),
    ("::1", 128),
    ("fc00::", 7),
    ("fe80::", 10),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotLocal {
    PublicAddress,
    WideWildcard,
    PublicName,
}

fn lan_ip(ip: IpAddr) -> bool {
    PERMITTED_NETS.iter().any(|(net, prefix)| {
        net.parse::<IpAddr>()
            .ok()
            .and_then(|n| Cidr::new(n, *prefix))
            .is_some_and(|c| c.contains(ip))
    })
}

fn under_lan_suffix(name: &str) -> bool {
    PERMITTED_NAMES.iter().any(|s| {
        name.strip_suffix(s)
            .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1)
    })
}

pub fn local_ca_refuses(host: &str) -> Option<NotLocal> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return (!lan_ip(ip)).then_some(NotLocal::PublicAddress);
    }
    if let Some(rest) = host.strip_prefix("*.") {
        return (!under_lan_suffix(rest)).then_some(NotLocal::WideWildcard);
    }
    (!is_localhost(host) && !under_lan_suffix(host)).then_some(NotLocal::PublicName)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_ca_signs_only_names_and_addresses_of_your_network() {
        for ok in [
            "nas.lan",
            "printer.home.arpa",
            "localhost",
            "*.nas.lan",
            "192.168.1.5",
            "fd00::5",
        ] {
            assert_eq!(local_ca_refuses(ok), None, "{ok}");
        }
        assert_eq!(
            local_ca_refuses("blog.example.com"),
            Some(NotLocal::PublicName)
        );
        assert_eq!(local_ca_refuses("lan"), Some(NotLocal::PublicName));
        assert_eq!(local_ca_refuses("*.lan"), Some(NotLocal::WideWildcard));
        assert_eq!(
            local_ca_refuses("203.0.113.10"),
            Some(NotLocal::PublicAddress)
        );
    }
}
