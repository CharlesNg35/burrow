use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::config::settings::Forward;
use crate::policy::allowlist::Rule;
use crate::wire::protocol::{Grants, VisitTarget};
use burrow_config::Protocol;

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

struct Ports {
    held: Vec<bool>,
    probed: Option<Vec<Option<bool>>>,
    kind: socket2::Type,
}

struct Spaces {
    tcp: Ports,
    udp: Ports,
}

impl Spaces {
    fn new(probe: bool) -> Spaces {
        Spaces {
            tcp: Ports::new(probe, socket2::Type::STREAM),
            udp: Ports::new(probe, socket2::Type::DGRAM),
        }
    }

    fn of(&mut self, protocol: Protocol) -> &mut Ports {
        match protocol {
            Protocol::Tcp => &mut self.tcp,
            Protocol::Udp => &mut self.udp,
        }
    }
}

impl Ports {
    fn new(probe: bool, kind: socket2::Type) -> Ports {
        Ports {
            held: vec![false; 65536],
            probed: probe.then(|| vec![None; 65536]),
            kind,
        }
    }

    fn hold(&mut self, first: u16, count: u16) {
        let first = usize::from(first);
        let end = (first + usize::from(count)).min(self.held.len());
        self.held[first..end].fill(true);
    }

    fn open(&mut self, port: usize) -> bool {
        let kind = self.kind;
        let Some(probed) = &mut self.probed else {
            return true;
        };
        *probed[port].get_or_insert_with(|| {
            crate::wire::transport::udp::std_socket((Ipv4Addr::LOCALHOST, port as u16).into(), kind)
                .is_ok()
        })
    }

    fn free(&mut self, first: u16, count: u16) -> Option<u16> {
        let last = 65536 - u32::from(count);
        let found = (u32::from(first)..=last)
            .chain(1024..u32::from(first).min(last + 1))
            .find(|&at| {
                let range = at as usize..at as usize + usize::from(count);
                !self.held[range.clone()].contains(&true) && range.into_iter().all(|p| self.open(p))
            })?;
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

fn with_protocol(mut v: Value, protocol: Protocol) -> Value {
    if protocol == Protocol::Udp
        && let Value::Object(map) = &mut v
        && let Some(local) = map.remove("local")
    {
        map.insert("protocol".into(), json!("udp"));
        map.insert("local".into(), local);
    }
    v
}

fn existing_value(f: &Forward) -> Value {
    with_protocol(shape(f), f.protocol)
}

fn shape(f: &Forward) -> Value {
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

fn granted(f: &Forward, grants: &Grants, tcp_only: bool) -> bool {
    match &f.to {
        VisitTarget::Service { .. } if tcp_only && f.protocol == Protocol::Udp => true,
        VisitTarget::Service { name, .. } => grants
            .services
            .iter()
            .any(|s| &s.name == name && s.protocol == f.protocol && f.ports.max(1) <= s.ports),
        VisitTarget::Client { client, target } => grants.clients.iter().any(|c| {
            &c.name == client
                && (f.socks || c.reach.is_empty() || within(target, f.ports, &c.reach))
        }),
    }
}

fn within(target: &str, ports: u16, reach: &[String]) -> bool {
    if reach.iter().any(|r| r.parse::<Rule>().is_err()) {
        return true;
    }
    let rules = crate::policy::allowlist::AllowList::from_rules(reach);
    if let Some(path) = crate::config::check::unix::path_of(target) {
        return rules.permits_socket(path);
    }
    match target.parse::<std::net::SocketAddr>() {
        Ok(first) => {
            let last = first.port().saturating_add(ports.max(1) - 1);
            rules.permits(first) && rules.permits(std::net::SocketAddr::new(first.ip(), last))
        }
        Err(_) => true,
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

fn only_sockets(reach: &[String]) -> Option<Vec<&str>> {
    let rules: Vec<Rule> = reach.iter().filter_map(|r| r.parse().ok()).collect();
    let sockets_only = !reach.is_empty()
        && rules.len() == reach.len()
        && rules.iter().all(|r| matches!(r, Rule::Socket(_)));
    sockets_only.then(|| {
        reach
            .iter()
            .filter_map(|r| r.trim().strip_prefix("unix:"))
            .filter(|p| !p.ends_with("/*"))
            .collect()
    })
}

pub fn plan(
    grants: &Grants,
    existing: &BTreeMap<String, Forward>,
    probe: bool,
    tcp_only: bool,
) -> Plan {
    let mut out = Plan::default();
    let mut spaces = Spaces::new(probe);
    for (name, f) in existing {
        if granted(f, grants, tcp_only) {
            spaces.of(f.protocol).hold(f.local.port(), f.ports.max(1));
            out.forwards.insert(name.clone(), existing_value(f));
        } else {
            out.removed.push(name.clone());
        }
    }
    let kept: Vec<&Forward> = existing
        .values()
        .filter(|f| granted(f, grants, tcp_only))
        .collect();
    let covers = |f: &&Forward, target: &str, client: Option<Protocol>| match (&f.to, client) {
        (VisitTarget::Service { name, .. }, Some(p)) => name == target && f.protocol == p,
        (VisitTarget::Client { client, .. }, None) => client == target,
        _ => false,
    };
    for s in &grants.services {
        if kept.iter().any(|f| covers(f, &s.name, Some(s.protocol))) {
            continue;
        }
        let moved = existing.values().find(|f| {
            matches!(&f.to, VisitTarget::Service { name, .. } if name == &s.name)
                && f.protocol != s.protocol
                && f.ports.max(1) == s.ports
        });
        let first = match (moved, s.port) {
            (Some(f), _) => f.local.port(),
            (None, 0) => 10000,
            (None, p) if p < 1024 => p + 10000,
            (None, p) => p,
        };
        let Some(local) = spaces.of(s.protocol).free(first, s.ports) else {
            out.unplaced.push(s.name.clone());
            continue;
        };
        let key = unique(&s.name, &out.forwards, "-service");
        out.forwards.insert(
            key.clone(),
            with_protocol(
                json!({ "service": s.name, "local": local_value(IpAddr::V4(Ipv4Addr::LOCALHOST), local, s.ports) }),
                s.protocol,
            ),
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
        if kept.iter().any(|f| covers(f, &c.name, None)) {
            continue;
        }
        if let Some(sockets) = only_sockets(&c.reach) {
            for path in sockets {
                let Some(local) = spaces.tcp.free(10000, 1) else {
                    out.unplaced.push(format!("{} ({path})", c.name));
                    continue;
                };
                let stem: String = Path::new(path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .chars()
                    .map(|ch| {
                        if ch.is_ascii_alphanumeric() || "-_".contains(ch) {
                            ch
                        } else {
                            '-'
                        }
                    })
                    .collect();
                let key = unique(&format!("{}-{stem}", c.name), &out.forwards, "-socket");
                out.forwards.insert(
                    key.clone(),
                    json!({ "client": c.name, "target": format!("unix:{path}"), "local": local }),
                );
                out.added.push(key);
            }
            continue;
        }
        let Some(local) = spaces.tcp.free(1080, 1) else {
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
mod tests;
