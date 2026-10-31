use std::collections::BTreeSet;
use std::net::IpAddr;

use super::allowlist::Sources;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Countries {
    Allow(BTreeSet<String>),
    Deny(BTreeSet<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Known(String),
    Unknown,
    NoDatabase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    In,
    Address,
    Region,
}

impl Verdict {
    pub fn admitted(self) -> bool {
        self == Verdict::In
    }
}

impl Countries {
    pub fn codes(&self) -> &BTreeSet<String> {
        match self {
            Countries::Allow(c) | Countries::Deny(c) => c,
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            Countries::Allow(_) => "allow_countries",
            Countries::Deny(_) => "deny_countries",
        }
    }

    fn lets_in(&self, place: &Place) -> bool {
        match (self, place) {
            (Countries::Allow(list), Place::Known(c)) => list.contains(c),
            (Countries::Allow(_), Place::Unknown | Place::NoDatabase) => false,
            (Countries::Deny(list), Place::Known(c)) => !list.contains(c),
            (Countries::Deny(_), Place::Unknown | Place::NoDatabase) => true,
        }
    }
}

pub fn judged_by_country(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || (a == 100 && (64..128).contains(&b)))
        }
        IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local())
        }
    }
}

pub fn admit(
    sources: &Sources,
    countries: Option<&Countries>,
    ip: IpAddr,
    place: impl FnOnce(IpAddr) -> Place,
) -> Verdict {
    let ip = ip.to_canonical();
    let address = |ok: bool| if ok { Verdict::In } else { Verdict::Address };
    if sources.deny.iter().any(|c| c.contains(ip)) {
        return Verdict::Address;
    }
    let Some(countries) = countries.filter(|_| judged_by_country(ip)) else {
        return address(sources.permits(ip));
    };
    if sources.allow.iter().any(|c| c.contains(ip)) {
        return Verdict::In;
    }
    if matches!(countries, Countries::Deny(_)) && !sources.allow.is_empty() {
        return Verdict::Address;
    }
    if countries.lets_in(&place(ip)) {
        Verdict::In
    } else {
        Verdict::Region
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn set(codes: &[&str]) -> BTreeSet<String> {
        codes.iter().map(|c| c.to_string()).collect()
    }

    fn sources(allow: &[&str], deny: &[&str]) -> Sources {
        Sources {
            allow: allow.iter().map(|c| c.parse().unwrap()).collect(),
            deny: deny.iter().map(|c| c.parse().unwrap()).collect(),
        }
    }

    fn at(code: &str) -> impl FnOnce(IpAddr) -> Place {
        let code = code.to_string();
        move |_| Place::Known(code)
    }

    #[test]
    fn allow_countries_lets_in_listed_countries_and_allow_from_addresses() {
        let fr = Countries::Allow(set(&["FR", "BE"]));
        let open = sources(&[], &[]);
        assert_eq!(
            admit(&open, Some(&fr), ip("203.0.113.5"), at("FR")),
            Verdict::In
        );
        assert_eq!(
            admit(&open, Some(&fr), ip("203.0.113.5"), at("US")),
            Verdict::Region
        );
        assert_eq!(
            admit(&open, Some(&fr), ip("203.0.113.5"), |_| Place::Unknown),
            Verdict::Region
        );
        assert_eq!(
            admit(&open, Some(&fr), ip("203.0.113.5"), |_| Place::NoDatabase),
            Verdict::Region
        );
        let office = sources(&["198.51.100.0/24"], &[]);
        assert_eq!(
            admit(&office, Some(&fr), ip("198.51.100.9"), at("US")),
            Verdict::In
        );
        assert_eq!(
            admit(&office, Some(&fr), ip("203.0.113.5"), at("BE")),
            Verdict::In
        );
    }

    #[test]
    fn deny_countries_refuses_listed_countries_and_keeps_allow_from_limits() {
        let ru = Countries::Deny(set(&["RU"]));
        let open = sources(&[], &[]);
        assert_eq!(
            admit(&open, Some(&ru), ip("203.0.113.5"), at("RU")),
            Verdict::Region
        );
        assert_eq!(
            admit(&open, Some(&ru), ip("203.0.113.5"), at("FR")),
            Verdict::In
        );
        assert_eq!(
            admit(&open, Some(&ru), ip("203.0.113.5"), |_| Place::Unknown),
            Verdict::In
        );
        assert_eq!(
            admit(&open, Some(&ru), ip("203.0.113.5"), |_| Place::NoDatabase),
            Verdict::In
        );
        let office = sources(&["198.51.100.0/24"], &[]);
        assert_eq!(
            admit(&office, Some(&ru), ip("198.51.100.9"), at("RU")),
            Verdict::In
        );
        assert_eq!(
            admit(&office, Some(&ru), ip("203.0.113.5"), at("FR")),
            Verdict::Address
        );
    }

    #[test]
    fn deny_from_always_wins_and_private_addresses_skip_the_country() {
        let fr = Countries::Allow(set(&["FR"]));
        let deny = sources(&["203.0.113.0/24"], &["203.0.113.5"]);
        assert_eq!(
            admit(&deny, Some(&fr), ip("203.0.113.5"), at("FR")),
            Verdict::Address
        );
        let open = sources(&[], &[]);
        for private in [
            "10.1.2.3",
            "192.168.1.1",
            "127.0.0.1",
            "::1",
            "fd00::1",
            "100.64.0.1",
            "::ffff:10.0.0.1",
        ] {
            let asked = std::cell::Cell::new(false);
            let place = |_| {
                asked.set(true);
                Place::Known("US".into())
            };
            assert_eq!(
                admit(&open, Some(&fr), ip(private), place),
                Verdict::In,
                "{private}"
            );
            assert!(!asked.get(), "{private}");
        }
        let lan = sources(&["10.0.0.0/8"], &[]);
        assert_eq!(
            admit(&lan, Some(&fr), ip("192.168.1.1"), at("FR")),
            Verdict::Address
        );
        assert_eq!(admit(&open, None, ip("203.0.113.5"), at("US")), Verdict::In);
    }
}
