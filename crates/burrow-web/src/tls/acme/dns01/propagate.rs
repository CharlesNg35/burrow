use std::time::{Duration, Instant};

use hickory_proto::op::{Message, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};

use super::lookup::{Lookup, Server};
use super::wire::{self, Failed, question, shown};

pub const POLL_EVERY: Duration = Duration::from_secs(3);

struct Target {
    fqdn: Name,
    values: Vec<String>,
    servers: Vec<(Server, Waiting)>,
}

#[derive(PartialEq, Eq)]
enum Waiting {
    Unasked,
    Ready,
    For(String),
}

enum Seen {
    Ready,
    NotYet(String),
    Wrong(String),
}

pub async fn wait_all(
    lookup: &Lookup<'_>,
    records: &[(Name, String)],
    limit: Duration,
    every: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + limit;
    let mut targets = discover(lookup, records).await?;
    loop {
        let mut pending = false;
        for t in &mut targets {
            for (server, state) in &mut t.servers {
                if *state == Waiting::Ready {
                    continue;
                }
                match check(lookup, server, &t.fqdn, &t.values).await {
                    Seen::Ready => *state = Waiting::Ready,
                    Seen::NotYet(why) => {
                        pending = true;
                        *state = Waiting::For(why);
                    }
                    Seen::Wrong(text) => return Err(text),
                }
            }
        }
        if !pending {
            return Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(timed_out(&targets, limit));
        }
        tokio::time::sleep(every.min(deadline - now)).await;
    }
}

async fn discover(lookup: &Lookup<'_>, records: &[(Name, String)]) -> Result<Vec<Target>, String> {
    let mut targets: Vec<Target> = Vec::new();
    for (fqdn, value) in records {
        if let Some(t) = targets.iter_mut().find(|t| t.fqdn == *fqdn) {
            if !t.values.contains(value) {
                t.values.push(value.clone());
            }
            continue;
        }
        let zone = lookup.zone_of(fqdn).await?;
        let servers = lookup.nameservers(&zone).await?;
        targets.push(Target {
            fqdn: fqdn.clone(),
            values: vec![value.clone()],
            servers: servers.into_iter().map(|s| (s, Waiting::Unasked)).collect(),
        });
    }
    Ok(targets)
}

fn timed_out(targets: &[Target], limit: Duration) -> String {
    let mut parts = Vec::new();
    for t in targets {
        let late: Vec<String> = t
            .servers
            .iter()
            .filter_map(|(s, state)| match state {
                Waiting::Ready => None,
                Waiting::For(why) => Some(format!("{} ({why})", shown(&s.host))),
                Waiting::Unasked => Some(shown(&s.host)),
            })
            .collect();
        if !late.is_empty() {
            parts.push(format!(
                "the TXT record {} did not appear on {} within {} s",
                shown(&t.fqdn),
                late.join(", "),
                limit.as_secs()
            ));
        }
    }
    parts.join("; ")
}

async fn check(lookup: &Lookup<'_>, server: &Server, fqdn: &Name, values: &[String]) -> Seen {
    let host = shown(&server.host);
    let mut answered = false;
    for ip in &server.addrs {
        let to = lookup.reach.nameserver(*ip);
        let m = match wire::ask(to, &question(fqdn, RecordType::TXT, false)).await {
            Ok(m) => m,
            Err(Failed::NoRoute(_)) => continue,
            Err(Failed::Silent(_) | Failed::Bad(_)) => {
                return Seen::NotYet("did not answer".into());
            }
        };
        if m.metadata.response_code == ResponseCode::Refused {
            return Seen::Wrong(format!(
                "the nameserver {host} ({ip}) refused the question for {}",
                shown(fqdn)
            ));
        }
        if !m.metadata.authoritative {
            return Seen::Wrong(format!(
                "the nameserver {host} ({ip}) answers for {} without authority; the zone's NS records point at a server that does not serve it",
                shown(fqdn)
            ));
        }
        let have = txt_values(&m, fqdn);
        if !values.iter().all(|v| have.contains(v)) {
            return Seen::NotYet("not yet".into());
        }
        answered = true;
    }
    if answered {
        Seen::Ready
    } else {
        Seen::NotYet("cannot be reached from here".into())
    }
}

pub fn txt_values(m: &Message, fqdn: &Name) -> Vec<String> {
    m.answers
        .iter()
        .filter(|r| r.name == *fqdn)
        .filter_map(|r| match &r.data {
            RData::TXT(t) => Some(
                t.txt_data
                    .iter()
                    .map(|d| String::from_utf8_lossy(d).into_owned())
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
