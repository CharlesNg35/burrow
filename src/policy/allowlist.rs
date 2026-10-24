use std::net::{IpAddr, SocketAddr};

pub use burrow_syntax::cidr::Cidr;
use burrow_syntax::cidr::canonical;

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
            AllowList::All => vec!["any".into()],
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
    probe.local_addr().ok().map(|a| canonical(a.ip()))
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
        let target = SocketAddr::new(canonical(target.ip()), target.port());
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
    let ip = canonical(ip);
    METADATA.contains(&ip)
        || match ip {
            IpAddr::V4(v4) => v4.is_link_local(),
            IpAddr::V6(v6) => v6.is_unicast_link_local(),
        }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rules_travel_as_text_and_a_bad_one_allows_nothing() {
        let given = [
            "192.168.150.0/25",
            "10.0.0.0/8:22",
            "10.1.0.0/16:8000-8100",
            "[fd00::/8]:443",
            "unix:/run/app.sock",
            "unix:/run/apps/*",
        ];
        let list = AllowList::Rules(given.iter().map(|r| r.parse().unwrap()).collect());
        let text = list.rules().unwrap();
        assert_eq!(AllowList::from_rules(&text), list);
        let bad = AllowList::from_rules(&["10.0.0.0/8".into(), "nonsense".into()]);
        assert!(!bad.permits("10.1.2.3:22".parse().unwrap()));
        assert!(!AllowList::from_rules(&[]).permits("10.1.2.3:22".parse().unwrap()));
        assert_eq!(AllowList::All.rules(), None);
    }

    #[test]
    fn a_long_allow_list_says_how_many_more_it_has() {
        let rules: Vec<Rule> = (0..100)
            .map(|i| format!("10.{i}.0.0/16").parse().unwrap())
            .collect();
        let told = AllowList::Rules(rules).describe();
        assert_eq!(told.len(), crate::wire::protocol::MAX_ALLOW_RULES);
        assert_eq!(told.last().unwrap(), "and 37 more");
        assert_eq!(AllowList::All.describe(), vec!["any"]);
    }

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
    fn socket_rules() {
        let list = AllowList::Rules(vec![
            rule("unix:/var/run/docker.sock"),
            rule("unix:/run/postgresql/*"),
            rule("127.0.0.0/8"),
        ]);
        assert!(list.permits_socket("/var/run/docker.sock"));
        assert!(!list.permits_socket("/var/run/docker.sock2"));
        assert!(list.permits_socket("/run/postgresql/.s.PGSQL.5432"));
        assert!(!list.permits_socket("/run/postgresql"));
        assert!(!list.permits_socket("/run/postgresql/sub/x"));
        assert!(!list.permits_socket("/run/postgresql/../docker.sock"));
        assert!(!list.permits_socket("/run/postgresql2/x"));
        assert!(!list.permits(sa("10.0.0.1:1")));
        assert!(list.permits(sa("127.0.0.1:1")));
        assert!(!AllowList::Rules(vec![rule("127.0.0.0/8")]).permits_socket("/tmp/x.sock"));
        assert!(AllowList::All.permits_socket("/tmp/x.sock"));
        assert!(!AllowList::All.permits_socket("relative.sock"));
        assert_eq!(
            rule("unix:/run/postgresql/*").to_string(),
            "unix:/run/postgresql/*"
        );
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
    }

    #[test]
    fn zero_prefix_matches_family() {
        let r = rule("0.0.0.0/0");
        assert!(r.matches(sa("8.8.8.8:1")));
        assert!(!r.matches(sa("[2001::1]:1")));
    }

    #[test]
    fn a_client_gate_is_its_allow_list() {
        let list = AllowList::Rules(vec![rule("127.0.0.0/8"), rule("169.254.0.0/16")]);
        let gate = Gate::client(list.clone());
        for t in [
            "127.0.0.1:22",
            "10.0.0.1:22",
            "169.254.169.254:80",
            "[fe80::1]:22",
        ] {
            assert_eq!(
                gate.permits(Protocol::Tcp, sa(t)),
                list.permits(sa(t)),
                "{t}"
            );
        }
        assert!(Gate::client(AllowList::All).permits(Protocol::Tcp, sa("127.0.0.1:7835")));
        assert_eq!(
            gate.permits_socket("/run/a.sock"),
            list.permits_socket("/run/a.sock")
        );
    }

    #[test]
    fn this_server_never_dials_its_own_listeners() {
        let own = vec![sa("0.0.0.0:7835"), sa("127.0.0.1:9100")];
        let gate = Gate::this_server(None, tcp(&own));
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("127.0.0.1:7835")),
            Some(Refusal::Burrow(sa("0.0.0.0:7835")))
        );
        assert_eq!(gate.refusal(Protocol::Tcp, sa("192.0.2.9:7835")), None);
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("[::ffff:127.0.0.1]:9100")),
            Some(Refusal::Burrow(sa("127.0.0.1:9100")))
        );
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("0.0.0.0:9100")),
            Some(Refusal::Burrow(sa("127.0.0.1:9100")))
        );
        assert_eq!(gate.refusal(Protocol::Tcp, sa("10.0.0.9:9100")), None);
        assert_eq!(gate.refusal(Protocol::Tcp, sa("127.0.0.1:5432")), None);
    }

    fn tcp(own: &[SocketAddr]) -> Vec<(Protocol, SocketAddr)> {
        own.iter().map(|a| (Protocol::Tcp, *a)).collect()
    }

    #[test]
    fn udp_targets_are_burrows_own_only_on_its_udp_binds() {
        let own = vec![
            (Protocol::Tcp, sa("0.0.0.0:7835")),
            (Protocol::Tcp, sa("127.0.0.1:443")),
            (Protocol::Tcp, sa("127.0.0.1:9100")),
            (Protocol::Udp, sa("0.0.0.0:5000")),
        ];
        let gate = Gate::this_server(None, own);
        for t in ["127.0.0.1:7835", "127.0.0.1:443", "127.0.0.1:9100"] {
            assert_eq!(gate.refusal(Protocol::Udp, sa(t)), None, "{t}");
        }
        assert_eq!(
            gate.refusal(Protocol::Udp, sa("127.0.0.1:5000")),
            Some(Refusal::Burrow(sa("0.0.0.0:5000")))
        );
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("127.0.0.1:7835")),
            Some(Refusal::Burrow(sa("0.0.0.0:7835")))
        );
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("127.0.0.1:9100")),
            Some(Refusal::Burrow(sa("127.0.0.1:9100")))
        );
        assert_eq!(gate.refusal(Protocol::Tcp, sa("127.0.0.1:5000")), None);
        assert!(!gate.permits(Protocol::Udp, sa("127.0.0.1:5000")));
        assert!(gate.permits(Protocol::Udp, sa("127.0.0.1:7835")));
    }

    #[test]
    fn a_wildcard_listener_covers_only_addresses_of_this_host() {
        let gate = Gate::this_server(None, tcp(&[sa("0.0.0.0:7835")]));
        assert_eq!(gate.refusal(Protocol::Tcp, sa("192.0.2.9:7835")), None);
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("127.0.0.1:7835")),
            Some(Refusal::Burrow(sa("0.0.0.0:7835")))
        );
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("0.0.0.0:7835")),
            Some(Refusal::Burrow(sa("0.0.0.0:7835")))
        );
        assert_eq!(gate.refusal(Protocol::Tcp, sa("[::1]:7835")), None);
        let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let routed = probe
            .connect("192.0.2.1:9")
            .and_then(|()| probe.local_addr())
            .map(|a| a.ip())
            .ok()
            .filter(|ip| !ip.is_loopback() && !ip.is_unspecified());
        match routed {
            Some(ip) => assert_eq!(
                gate.refusal(Protocol::Tcp, SocketAddr::new(ip, 7835)),
                Some(Refusal::Burrow(sa("0.0.0.0:7835"))),
                "{ip}"
            ),
            None => assert!(
                std::env::var_os("CI").is_none(),
                "a CI runner has a routed address to check as this host's own"
            ),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_host_that_binds_any_address_still_tells_its_own_from_another_machine() {
        const NAME: &str = "policy::allowlist::tests::a_host_that_binds_any_address_still_tells_its_own_from_another_machine";
        const SETUP: &str = "ip link set lo up && ip addr add 10.9.9.9/24 dev lo && ip addr add fd09::9/64 dev lo nodad && echo 1 > /proc/sys/net/ipv4/ip_nonlocal_bind && echo 1 > /proc/sys/net/ipv6/ip_nonlocal_bind";
        if std::env::var_os("BURROW_NONLOCAL_BIND_NETNS").is_some() {
            assert!(std::net::UdpSocket::bind("10.9.9.20:0").is_ok());
            assert!(std::net::UdpSocket::bind("[fd09::20]:0").is_ok());
            let gate = Gate::this_server(None, tcp(&[sa("0.0.0.0:7835"), sa("[::]:7836")]));
            for other in ["10.9.9.20:7835", "192.0.2.9:7835", "[fd09::20]:7836"] {
                assert_eq!(gate.refusal(Protocol::Tcp, sa(other)), None, "{other}");
            }
            assert_eq!(
                gate.refusal(Protocol::Tcp, sa("10.9.9.9:7835")),
                Some(Refusal::Burrow(sa("0.0.0.0:7835")))
            );
            assert_eq!(
                gate.refusal(Protocol::Tcp, sa("[fd09::9]:7836")),
                Some(Refusal::Burrow(sa("[::]:7836")))
            );
            return;
        }
        let netns = |script: String| {
            std::process::Command::new("unshare")
                .args(["-rn", "sh", "-c", &script])
                .env("BURROW_NONLOCAL_BIND_NETNS", "1")
                .output()
        };
        if !netns(SETUP.to_string()).is_ok_and(|o| o.status.success()) {
            assert!(
                std::env::var_os("CI").is_none(),
                "a CI runner permits a private network namespace"
            );
            println!("skipped: this machine does not permit a private network namespace");
            return;
        }
        let exe = std::env::current_exe().unwrap();
        let out = netns(format!(
            "{SETUP} && exec '{}' --exact {NAME} --test-threads 1",
            exe.display()
        ))
        .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && said.contains("1 passed"),
            "{said}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn a_wildcard_ipv6_listener_covers_loopback_of_both_families() {
        if std::net::UdpSocket::bind("[::1]:0").is_err() {
            return;
        }
        let gate = Gate::this_server(None, tcp(&[sa("[::]:7835")]));
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("[::1]:7835")),
            Some(Refusal::Burrow(sa("[::]:7835")))
        );
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("127.0.0.1:7835")),
            Some(Refusal::Burrow(sa("[::]:7835")))
        );
        assert_eq!(gate.refusal(Protocol::Tcp, sa("[2001:db8::9]:7835")), None);
        assert_eq!(gate.refusal(Protocol::Tcp, sa("192.0.2.9:7835")), None);
    }

    #[test]
    fn this_server_reaches_link_local_only_by_a_rule_naming_the_address() {
        let wide = AllowList::Rules(vec![rule("0.0.0.0/0"), rule("::/0")]);
        let gate = Gate::this_server(Some(&wide), Vec::new());
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("169.254.169.254:80")),
            Some(Refusal::LinkLocal)
        );
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("[fe80::1]:22")),
            Some(Refusal::LinkLocal)
        );
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("[::ffff:169.254.1.1]:22")),
            Some(Refusal::LinkLocal)
        );
        assert_eq!(
            Gate::this_server(None, Vec::new()).refusal(Protocol::Tcp, sa("169.254.169.254:80")),
            Some(Refusal::LinkLocal)
        );
        let named = AllowList::Rules(vec![rule("169.254.169.254/32:80"), rule("fe80::1")]);
        let gate = Gate::this_server(Some(&named), Vec::new());
        assert_eq!(gate.refusal(Protocol::Tcp, sa("169.254.169.254:80")), None);
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("169.254.169.254:81")),
            Some(Refusal::LinkLocal)
        );
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("169.254.169.253:80")),
            Some(Refusal::LinkLocal)
        );
        assert_eq!(gate.refusal(Protocol::Tcp, sa("[fe80::1]:22")), None);
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("10.0.0.1:22")),
            Some(Refusal::Outside)
        );
    }

    #[test]
    fn cloud_metadata_addresses_are_reached_only_by_a_rule_naming_them() {
        let wide = AllowList::Rules(vec![rule("fd00::/8"), rule("100.64.0.0/10")]);
        let gate = Gate::this_server(Some(&wide), Vec::new());
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("[fd00:ec2::254]:80")),
            Some(Refusal::LinkLocal)
        );
        assert_eq!(
            gate.refusal(Protocol::Tcp, sa("100.100.100.200:80")),
            Some(Refusal::LinkLocal)
        );
        assert_eq!(gate.refusal(Protocol::Tcp, sa("[fd00:ec2::253]:80")), None);
        assert_eq!(gate.refusal(Protocol::Tcp, sa("100.100.100.201:80")), None);
        let named = AllowList::Rules(vec![rule("fd00:ec2::254"), rule("100.100.100.200/32:80")]);
        let gate = Gate::this_server(Some(&named), Vec::new());
        assert_eq!(gate.refusal(Protocol::Tcp, sa("[fd00:ec2::254]:80")), None);
        assert_eq!(gate.refusal(Protocol::Tcp, sa("100.100.100.200:80")), None);
        assert!(Gate::client(wide).permits(Protocol::Tcp, sa("100.100.100.200:80")));
    }
}
