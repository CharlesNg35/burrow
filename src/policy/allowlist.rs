use std::net::{IpAddr, SocketAddr};

pub use burrow_syntax::cidr::Cidr;

use burrow_config::Protocol;
pub use burrow_syntax::rule::{NetRule, Rule, RuleError, SocketRule};

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

    pub fn describe(&self) -> Vec<String> {
        let max = crate::wire::protocol::MAX_ALLOW_RULES;
        match self {
            AllowList::All => vec![burrow_syntax::rule::ANY.into()],
            AllowList::Rules(rules) => {
                let shown = |r: &Rule| {
                    let text = r.to_string();
                    if text.len() <= 200 {
                        text
                    } else {
                        let cut: String = text.chars().take(190).collect();
                        format!("{cut}…")
                    }
                };
                if rules.len() <= max {
                    return rules.iter().map(shown).collect();
                }
                let mut out: Vec<String> = rules.iter().take(max - 1).map(shown).collect();
                out.push(format!("and {} more", rules.len() - (max - 1)));
                out
            }
        }
    }

    pub fn rules(&self) -> Option<Vec<String>> {
        match self {
            AllowList::All => None,
            AllowList::Rules(rules) => Some(rules.iter().map(Rule::to_string).collect()),
        }
    }

    pub fn from_rules(given: &[String]) -> AllowList {
        let parsed: Option<Vec<Rule>> = given.iter().map(|r| r.parse().ok()).collect();
        AllowList::Rules(parsed.unwrap_or_default())
    }

    pub fn permits_socket(&self, path: &str) -> bool {
        match self {
            AllowList::All => burrow_syntax::unix::check_path(path).is_ok(),
            AllowList::Rules(rules) => rules.iter().any(|r| r.matches_socket(path)),
        }
    }

    pub fn range_suggestion(ip: IpAddr, first: u16, last: u16) -> String {
        match ip.to_canonical() {
            IpAddr::V4(ip) => format!("{ip}/32:{first}-{last}"),
            IpAddr::V6(ip) => format!("[{ip}/128]:{first}-{last}"),
        }
    }

    pub fn suggestion(target: SocketAddr) -> String {
        match target.ip().to_canonical() {
            IpAddr::V4(ip) => format!("{ip}/32:{}", target.port()),
            IpAddr::V6(ip) => format!("[{ip}/128]:{}", target.port()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Burrow(SocketAddr),
    LinkLocal,
    Outside,
}

fn listens_on(own: (Protocol, SocketAddr), protocol: Protocol, target: SocketAddr) -> bool {
    let (own_protocol, own) = own;
    own_protocol == protocol
        && burrow_syntax::ports::binds_conflict(own, target)
        && (!own.ip().is_unspecified() || is_this_host(target.ip()))
}

fn is_this_host(ip: IpAddr) -> bool {
    ip.is_unspecified() || ip.is_loopback() || source_towards(ip) == Some(ip)
}

fn source_towards(ip: IpAddr) -> Option<IpAddr> {
    let any = match ip {
        IpAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
        IpAddr::V6(_) => SocketAddr::from(([0u16; 8], 0)),
    };
    let probe = std::net::UdpSocket::bind(any).ok()?;
    probe.connect(SocketAddr::new(ip, 9)).ok()?;
    probe.local_addr().ok().map(|a| a.ip().to_canonical())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    allow: AllowList,
    own: Vec<(Protocol, SocketAddr)>,
    link_local_named_only: bool,
}

impl Gate {
    pub fn client(allow: AllowList) -> Gate {
        Gate {
            allow,
            own: Vec::new(),
            link_local_named_only: false,
        }
    }

    pub fn this_server(allow: Option<&AllowList>, own: Vec<(Protocol, SocketAddr)>) -> Gate {
        Gate {
            allow: allow.cloned().unwrap_or(AllowList::All),
            own,
            link_local_named_only: true,
        }
    }

    pub fn allow(&self) -> &AllowList {
        &self.allow
    }

    pub fn refusal(&self, protocol: Protocol, target: SocketAddr) -> Option<Refusal> {
        let target = SocketAddr::new(target.ip().to_canonical(), target.port());
        if let Some((_, own)) = self
            .own
            .iter()
            .find(|own| listens_on(**own, protocol, target))
        {
            return Some(Refusal::Burrow(*own));
        }
        if self.link_local_named_only && named_only(target.ip()) && !self.names(target) {
            return Some(Refusal::LinkLocal);
        }
        (!self.allow.permits(target)).then_some(Refusal::Outside)
    }

    pub fn permits(&self, protocol: Protocol, target: SocketAddr) -> bool {
        self.refusal(protocol, target).is_none()
    }

    pub fn permits_socket(&self, path: &str) -> bool {
        self.allow.permits_socket(path)
    }

    fn names(&self, target: SocketAddr) -> bool {
        let AllowList::Rules(rules) = &self.allow else {
            return false;
        };
        rules.iter().any(|r| match r {
            Rule::Net(net) => {
                let single = match net.net.addr() {
                    IpAddr::V4(_) => 32,
                    IpAddr::V6(_) => 128,
                };
                net.net.prefix() == single && net.matches(target)
            }
            Rule::Socket(_) => false,
        })
    }
}

const METADATA: [IpAddr; 2] = [
    IpAddr::V6(std::net::Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254)),
    IpAddr::V4(std::net::Ipv4Addr::new(100, 100, 100, 200)),
];

pub fn named_only(ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    METADATA.contains(&ip)
        || match ip {
            IpAddr::V4(v4) => v4.is_link_local(),
            IpAddr::V6(v6) => v6.is_unicast_link_local(),
        }
}

#[cfg(test)]
mod tests;
