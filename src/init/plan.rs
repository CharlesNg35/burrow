use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

use serde_json::{Map, Value, json};

use crate::config::settings::Forward;
use crate::protocol::{Grants, VisitTarget};

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub forwards: Map<String, Value>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub socks: Vec<(String, String, u16)>,
    pub allows: BTreeMap<String, Option<Vec<String>>>,
    pub unplaced: Vec<String>,
}

impl Plan {
    pub fn changed(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty()
    }
}

struct Ports(Vec<bool>);

impl Ports {
    fn new() -> Ports {
        Ports(vec![false; 65536])
    }

    fn hold(&mut self, first: u16, count: u16) {
        let first = usize::from(first);
        let end = (first + usize::from(count)).min(self.0.len());
        self.0[first..end].fill(true);
    }

    fn free(&mut self, first: u16, count: u16) -> Option<u16> {
        let last = 65536 - u32::from(count);
        let fits = |at: u32| {
            let at = at as usize;
            !self.0[at..at + usize::from(count)].contains(&true)
        };
        let found = (u32::from(first)..=last)
            .chain(1024..u32::from(first).min(last + 1))
            .find(|&at| fits(at))?;
        let local = u16::try_from(found).ok()?;
        self.hold(local, count);
        Some(local)
    }
}

fn local_value(at: IpAddr, port: u16, count: u16) -> Value {
    let ports = if count > 1 {
        crate::config::check::ports::show(port, count)
    } else {
        port.to_string()
    };
    match (at == IpAddr::V4(Ipv4Addr::LOCALHOST), count > 1) {
        (true, false) => json!(port),
        (true, true) => Value::String(ports),
        (false, _) => Value::String(match at {
            IpAddr::V6(ip) => format!("[{ip}]:{ports}"),
            IpAddr::V4(ip) => format!("{ip}:{ports}"),
        }),
    }
}

fn existing_value(f: &Forward) -> Value {
    let local = local_value(f.local.ip(), f.local.port(), f.ports.max(1));
    match &f.to {
        VisitTarget::Service { name, .. } => json!({ "service": name, "local": local }),
        VisitTarget::Client { client, .. } if f.socks => {
            json!({ "client": client, "socks": true, "local": local })
        }
        VisitTarget::Client { client, target } => {
            let target = match target.rsplit_once(':') {
                Some((host, port)) if f.ports > 1 && !target.starts_with("unix:") => {
                    match port.parse::<u16>() {
                        Ok(first) => format!(
                            "{host}:{}",
                            crate::config::check::ports::show(first, f.ports)
                        ),
                        Err(_) => target.clone(),
                    }
                }
                _ => target.clone(),
            };
            json!({ "client": client, "target": target, "local": local })
        }
    }
}

fn granted(f: &Forward, grants: &Grants) -> bool {
    match &f.to {
        VisitTarget::Service { name, .. } => grants
            .services
            .iter()
            .any(|s| &s.name == name && f.ports.max(1) <= s.ports),
        VisitTarget::Client { client, .. } => grants.clients.iter().any(|c| &c.name == client),
    }
}

fn unique(base: &str, taken: &Map<String, Value>, suffix: &str) -> String {
    let named = |suffix: String| {
        let keep = crate::config::check::MAX_NAME_LEN.saturating_sub(suffix.len());
        let base: String = base.chars().take(keep).collect();
        format!("{base}{suffix}")
    };
    std::iter::once(base.to_string())
        .chain(std::iter::once(named(suffix.to_string())))
        .chain((2..).map(|i| named(format!("{suffix}-{i}"))))
        .find(|k| !taken.contains_key(k.as_str()))
        .expect("an unused forward name")
}

pub fn plan(grants: &Grants, existing: &BTreeMap<String, Forward>) -> Plan {
    let mut out = Plan::default();
    let mut ports = Ports::new();
    for (name, f) in existing {
        if granted(f, grants) {
            ports.hold(f.local.port(), f.ports.max(1));
            out.forwards.insert(name.clone(), existing_value(f));
        } else {
            out.removed.push(name.clone());
        }
    }
    let kept: Vec<&Forward> = existing.values().filter(|f| granted(f, grants)).collect();
    let covers = |f: &&Forward, target: &str, client: bool| match (&f.to, client) {
        (VisitTarget::Service { name, .. }, false) => name == target,
        (VisitTarget::Client { client, .. }, true) => client == target,
        _ => false,
    };
    for s in &grants.services {
        if kept.iter().any(|f| covers(f, &s.name, false)) {
            continue;
        }
        let first = match s.port {
            0 => 10000,
            p if p < 1024 => p + 10000,
            p => p,
        };
        let Some(local) = ports.free(first, s.ports) else {
            out.unplaced.push(s.name.clone());
            continue;
        };
        let key = unique(&s.name, &out.forwards, "-service");
        out.forwards.insert(
            key.clone(),
            json!({ "service": s.name, "local": local_value(IpAddr::V4(Ipv4Addr::LOCALHOST), local, s.ports) }),
        );
        out.added.push(key);
    }
    for c in &grants.clients {
        let socks = existing
            .iter()
            .find(|(_, f)| {
                f.socks && matches!(&f.to, VisitTarget::Client { client, .. } if client == &c.name)
            })
            .map(|(name, _)| name.clone());
        if let Some(name) = socks {
            out.allows.insert(name, c.allow.clone());
            continue;
        }
        if kept.iter().any(|f| covers(f, &c.name, true)) {
            continue;
        }
        let Some(local) = ports.free(1080, 1) else {
            out.unplaced.push(c.name.clone());
            continue;
        };
        let key = unique(&c.name, &out.forwards, "-socks");
        out.forwards.insert(
            key.clone(),
            json!({ "client": c.name, "socks": true, "local": local }),
        );
        out.socks.push((key.clone(), c.name.clone(), local));
        out.allows.insert(key.clone(), c.allow.clone());
        out.added.push(key);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forward_to_part_of_a_range_is_kept_and_client_ranges_stay_ranges() {
        let mut existing = BTreeMap::new();
        existing.insert(
            "few".to_string(),
            Forward {
                ports: 3,
                ..Forward::single(at(40000), VisitTarget::service("game"))
            },
        );
        existing.insert(
            "lan".to_string(),
            Forward {
                ports: 11,
                ..Forward::single(
                    at(30000),
                    VisitTarget::Client {
                        client: "home".into(),
                        target: "127.0.0.1:30000".into(),
                    },
                )
            },
        );
        let p = plan(&grants(&[("game", 40000, 11)], &["home"]), &existing);
        assert!(!p.changed(), "{:?} {:?}", p.added, p.removed);
        assert_eq!(
            p.forwards["lan"],
            json!({ "client": "home", "target": "127.0.0.1:30000-30010", "local": "30000-30010" })
        );
    }
    use crate::protocol::{GrantedClient, GrantedService};
    use std::net::SocketAddr;

    fn grants(services: &[(&str, u16, u16)], clients: &[&str]) -> Grants {
        Grants {
            services: services
                .iter()
                .map(|(n, port, ports)| GrantedService {
                    name: n.to_string(),
                    port: *port,
                    ports: *ports,
                })
                .collect(),
            clients: clients
                .iter()
                .map(|c| GrantedClient {
                    name: c.to_string(),
                    reach: vec![],
                    allow: Some(vec!["127.0.0.0/8".into()]),
                })
                .collect(),
        }
    }

    #[test]
    fn grants_that_do_not_fit_are_left_out_instead_of_hanging() {
        let p = plan(
            &grants(
                &[("a", 30000, 30000), ("b", 31000, 30000), ("c", 5432, 1)],
                &["home"],
            ),
            &BTreeMap::new(),
        );
        assert_eq!(p.unplaced, vec!["b"]);
        assert_eq!(p.forwards["a"]["local"], "30000-59999");
        assert_eq!(p.forwards["c"]["local"], 5432);
        assert_eq!(p.forwards["home"]["local"], 1080);
        let many: Vec<(String, u16, u16)> =
            (0..70).map(|i| (format!("s{i}"), 20000, 1000)).collect();
        let many: Vec<(&str, u16, u16)> =
            many.iter().map(|(n, p, c)| (n.as_str(), *p, *c)).collect();
        let p = plan(&grants(&many, &[]), &BTreeMap::new());
        assert_eq!(p.forwards.len() + p.unplaced.len(), 70);
        assert!(!p.unplaced.is_empty());
        let mut seen = std::collections::HashSet::new();
        for v in p.forwards.values() {
            let (a, b) = v["local"].as_str().unwrap().split_once('-').unwrap();
            let (a, b): (u32, u32) = (a.parse().unwrap(), b.parse().unwrap());
            assert!(a >= 1024 && b <= 65535 && b - a == 999);
            assert!((a..=b).all(|p| seen.insert(p)), "overlap at {a}");
        }
    }

    fn at(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn a_new_config_gets_a_forward_for_each_grant() {
        let p = plan(
            &grants(
                &[("db", 5432, 1), ("ssh", 22, 1), ("game", 40000, 11)],
                &["home"],
            ),
            &BTreeMap::new(),
        );
        assert_eq!(p.forwards["db"], json!({ "service": "db", "local": 5432 }));
        assert_eq!(
            p.forwards["ssh"],
            json!({ "service": "ssh", "local": 10022 })
        );
        assert_eq!(
            p.forwards["game"],
            json!({ "service": "game", "local": "40000-40010" })
        );
        assert_eq!(
            p.forwards["home"],
            json!({ "client": "home", "socks": true, "local": 1080 })
        );
        assert_eq!(p.added, vec!["db", "ssh", "game", "home"]);
        assert!(p.removed.is_empty());
    }

    #[test]
    fn an_update_keeps_your_ports_adds_new_grants_and_drops_revoked_ones() {
        let mut existing = BTreeMap::new();
        existing.insert(
            "mydb".to_string(),
            Forward::single(at(6543), VisitTarget::service("db")),
        );
        existing.insert(
            "old".to_string(),
            Forward::single(at(7000), VisitTarget::service("gone")),
        );
        existing.insert(
            "ssh-home".to_string(),
            Forward::single(
                at(2222),
                VisitTarget::Client {
                    client: "home".into(),
                    target: "127.0.0.1:22".into(),
                },
            ),
        );
        let p = plan(
            &grants(&[("db", 5432, 1), ("web", 6543, 1)], &["home", "office"]),
            &existing,
        );
        assert_eq!(
            p.forwards["mydb"],
            json!({ "service": "db", "local": 6543 })
        );
        assert_eq!(
            p.forwards["web"],
            json!({ "service": "web", "local": 6544 }),
            "6543 is already yours"
        );
        assert_eq!(
            p.forwards["ssh-home"],
            json!({ "client": "home", "target": "127.0.0.1:22", "local": 2222 })
        );
        assert!(
            !p.forwards.contains_key("home"),
            "home is already reached through ssh-home"
        );
        assert_eq!(
            p.forwards["office"],
            json!({ "client": "office", "socks": true, "local": 1080 })
        );
        assert_eq!(p.added, vec!["web", "office"]);
        assert_eq!(p.removed, vec!["old"]);
    }

    #[test]
    fn nothing_changes_when_the_grants_did_not() {
        let first = plan(&grants(&[("db", 5432, 1)], &["home"]), &BTreeMap::new());
        let mut existing = BTreeMap::new();
        existing.insert(
            "db".to_string(),
            Forward::single(at(5432), VisitTarget::service("db")),
        );
        existing.insert("home".to_string(), Forward::socks(at(1080), "home".into()));
        let again = plan(&grants(&[("db", 5432, 1)], &["home"]), &existing);
        assert!(!again.changed());
        assert_eq!(again.forwards, first.forwards);
        assert_eq!(again.allows["home"], Some(vec!["127.0.0.0/8".to_string()]));
    }

    #[test]
    fn names_stay_unique_and_a_range_larger_than_granted_is_replaced() {
        let mut existing = BTreeMap::new();
        existing.insert(
            "game".to_string(),
            Forward {
                ports: 15,
                ..Forward::single(at(40000), VisitTarget::service("game"))
            },
        );
        let p = plan(
            &grants(&[("game", 40000, 11), ("home", 22, 1)], &["home"]),
            &existing,
        );
        assert_eq!(p.removed, vec!["game"]);
        assert_eq!(
            p.forwards["game"],
            json!({ "service": "game", "local": "40000-40010" })
        );
        assert_eq!(
            p.forwards["home"],
            json!({ "service": "home", "local": 10022 })
        );
        assert_eq!(
            p.forwards["home-socks"],
            json!({ "client": "home", "socks": true, "local": 1080 })
        );
    }
}
