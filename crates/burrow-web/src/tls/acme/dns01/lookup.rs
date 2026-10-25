use std::net::{IpAddr, SocketAddr};

use hickory_proto::op::{Message, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};

use super::wire::{self, Reach, question, shown};

pub const MAX_CNAMES: usize = 8;
pub const MAX_NAMESERVERS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub host: Name,
    pub addrs: Vec<IpAddr>,
}

pub struct Lookup<'a> {
    pub resolvers: &'a [SocketAddr],
    pub reach: &'a Reach,
}

impl Lookup<'_> {
    pub async fn recursive(&self, n: &Name, kind: RecordType) -> Result<Message, String> {
        let mut last = None;
        for r in self.resolvers {
            match wire::ask(*r, &question(n, kind, true)).await {
                Ok(m)
                    if matches!(
                        m.metadata.response_code,
                        ResponseCode::NoError | ResponseCode::NXDomain
                    ) =>
                {
                    return Ok(m);
                }
                Ok(m) => last = Some(format!("{r} answered {}", m.metadata.response_code)),
                Err(e) => last = Some(e.text().to_string()),
            }
        }
        Err(format!(
            "no resolver answered the {kind} question for {}: {}",
            shown(n),
            last.unwrap_or_else(|| "none is set".into())
        ))
    }

    pub async fn follow(&self, start: &Name) -> Result<Name, String> {
        let mut chain = vec![start.clone()];
        loop {
            let cur = chain.last().cloned().unwrap_or_else(|| start.clone());
            let m = self.recursive(&cur, RecordType::CNAME).await?;
            let next = m.answers.iter().find_map(|r| match &r.data {
                RData::CNAME(c) if r.name == cur => Some(c.0.clone()),
                _ => None,
            });
            let Some(next) = next else {
                return Ok(cur);
            };
            let looped = chain.contains(&next);
            chain.push(next);
            let path = || chain.iter().map(shown).collect::<Vec<_>>().join(" → ");
            if looped {
                return Err(format!("{} is a CNAME loop: {}", shown(start), path()));
            }
            if chain.len() > MAX_CNAMES + 1 {
                return Err(format!(
                    "{} follows more than {MAX_CNAMES} CNAMEs ({}); point it at the final name",
                    shown(start),
                    path()
                ));
            }
        }
    }

    pub async fn zone_of(&self, n: &Name) -> Result<Name, String> {
        for cand in suffixes(n) {
            let m = self.recursive(&cand, RecordType::SOA).await?;
            if has_soa(&m, &cand) {
                return Ok(cand);
            }
        }
        Err(format!("found no DNS zone that holds {}", shown(n)))
    }

    pub async fn nameservers(&self, zone: &Name) -> Result<Vec<Server>, String> {
        let m = self.recursive(zone, RecordType::NS).await?;
        let mut hosts: Vec<Name> = Vec::new();
        for r in &m.answers {
            if let RData::NS(ns) = &r.data
                && r.name == *zone
                && !hosts.contains(&ns.0)
            {
                hosts.push(ns.0.clone());
            }
        }
        hosts.truncate(MAX_NAMESERVERS);
        if hosts.is_empty() {
            return Err(format!("found no nameservers for the zone {}", shown(zone)));
        }
        let mut out = Vec::new();
        for host in hosts {
            let mut addrs = Vec::new();
            for kind in [RecordType::A, RecordType::AAAA] {
                let m = self.recursive(&host, kind).await?;
                addrs.extend(m.answers.iter().filter_map(|r| match &r.data {
                    RData::A(a) => Some(IpAddr::V4(a.0)),
                    RData::AAAA(a) => Some(IpAddr::V6(a.0)),
                    _ => None,
                }));
            }
            if addrs.is_empty() {
                return Err(format!(
                    "the nameserver {} of {} has no address",
                    shown(&host),
                    shown(zone)
                ));
            }
            out.push(Server { host, addrs });
        }
        Ok(out)
    }
}

pub async fn authoritative_zone(server: SocketAddr, n: &Name) -> Result<Option<Name>, String> {
    for cand in suffixes(n) {
        let m = wire::ask(server, &question(&cand, RecordType::SOA, false))
            .await
            .map_err(|e| e.text().to_string())?;
        if m.metadata.response_code == ResponseCode::Refused {
            return Err(format!(
                "{server} refused the question for {}: it may not serve that zone",
                shown(&cand)
            ));
        }
        if m.metadata.authoritative && has_soa(&m, &cand) {
            return Ok(Some(cand));
        }
    }
    Ok(None)
}

fn has_soa(m: &Message, zone: &Name) -> bool {
    m.answers
        .iter()
        .any(|r| r.name == *zone && matches!(r.data, RData::SOA(_)))
}

pub fn suffixes(n: &Name) -> Vec<Name> {
    let mut out = Vec::new();
    let mut cur = n.clone();
    while cur.num_labels() > 0 {
        out.push(cur.clone());
        cur = cur.base_name();
    }
    out
}

#[cfg(test)]
mod tests;
