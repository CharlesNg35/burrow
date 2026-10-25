use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use hickory_proto::rr::{Name, RData, RecordType};
use tracing::warn;

use super::lookup::Lookup;
use super::pending::{self, Left};
use super::wire::{Reach, name, shown};
use super::{DnsEntry, DnsProvider, Provider, Refusal, entry_for};
use crate::tls::acme::http::{Client, Limits};
use crate::tls::now_secs;

pub struct Dns {
    entries: Vec<DnsEntry>,
    providers: Vec<Result<Box<dyn Provider>, String>>,
    reach: Reach,
    state: Option<PathBuf>,
    system: Vec<SocketAddr>,
    pub every: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Set {
    pub name: String,
    pub entry: usize,
    pub fqdn: Name,
    pub value: String,
}

enum Change {
    Add(Left),
    Remove(String, String),
}

pub fn challenge_name(identifier: &str) -> Result<Name, String> {
    let bare = identifier
        .trim()
        .trim_start_matches("*.")
        .trim_end_matches('.');
    name(&format!("_acme-challenge.{bare}"))
}

fn build(p: &DnsProvider, reach: &Reach) -> Result<Box<dyn Provider>, String> {
    let client = || Client::new(reach.api_ca.as_deref(), Limits::default());
    Ok(match p {
        DnsProvider::Cloudflare { token } => Box::new(super::cloudflare::Cloudflare::new(
            token.clone(),
            reach.cloudflare_api.as_deref(),
            client()?,
        )),
        DnsProvider::Rfc2136 {
            server,
            key_name,
            secret,
        } => Box::new(super::rfc2136::Rfc2136::new(
            server,
            key_name,
            secret.clone(),
        )?),
        DnsProvider::AcmeDns {
            api,
            username,
            password,
            subdomain,
        } => Box::new(super::acme_dns::AcmeDns::new(
            api,
            username,
            password.clone(),
            subdomain,
            client()?,
        )),
        DnsProvider::Command { path } => Box::new(super::command::Command::new(path.clone())),
    })
}

fn parent(n: &str) -> &str {
    match n.strip_prefix("*.") {
        Some(rest) => rest,
        None => n.split_once('.').map_or(n, |(_, rest)| rest),
    }
}

impl Dns {
    pub fn new(entries: &[DnsEntry], reach: Reach, state: Option<PathBuf>) -> Dns {
        let providers = entries.iter().map(|e| build(&e.provider, &reach)).collect();
        Dns {
            entries: entries.to_vec(),
            providers,
            reach,
            state,
            system: super::system::system().addrs,
            every: super::propagate::POLL_EVERY,
        }
    }

    pub fn with_system(mut self, resolvers: Vec<SocketAddr>) -> Dns {
        self.system = resolvers;
        self
    }

    fn lookup(&self, entry: usize) -> Lookup<'_> {
        let own = self
            .entries
            .get(entry)
            .map(|e| e.resolvers.as_slice())
            .unwrap_or(&[]);
        Lookup {
            resolvers: if own.is_empty() { &self.system } else { own },
            reach: &self.reach,
        }
    }

    fn provider(&self, entry: usize) -> Result<&dyn Provider, String> {
        match self.providers.get(entry) {
            Some(Ok(p)) => Ok(p.as_ref()),
            Some(Err(e)) => Err(format!("web.letsencrypt.dns[{entry}]: {e}")),
            None => Err(format!("web.letsencrypt.dns[{entry}] is gone")),
        }
    }

    fn pending_path(&self) -> Option<PathBuf> {
        self.state.as_deref().map(pending::path)
    }

    pub async fn present(
        &self,
        items: &[(String, String)],
        set: &mut Vec<Set>,
    ) -> Result<(), String> {
        for (ident, value) in items {
            let (entry, _) = entry_for(&self.entries, ident).ok_or_else(|| {
                format!(
                    "{ident} needs a DNS check, but no web.letsencrypt.dns entry lists its domain"
                )
            })?;
            let provider = self.provider(entry)?;
            let start = challenge_name(ident)?;
            let lookup = self.lookup(entry);
            let fqdn = lookup
                .follow(&start)
                .await
                .map_err(|e| format!("{ident}: {e}"))?;
            let left = Left {
                name: ident.clone(),
                fqdn: fqdn.to_ascii(),
                value: value.clone(),
                since: now_secs(),
            };
            self.note(Change::Add(left.clone())).await;
            let done = provider.present(&fqdn, value).await;
            if let Err(Refusal::NotWritable(why)) = &done {
                self.note(Change::Remove(left.fqdn.clone(), value.clone()))
                    .await;
                return Err(self.not_writable(entry, ident, &start, &fqdn, why).await);
            }
            set.push(Set {
                name: ident.clone(),
                entry,
                fqdn,
                value: value.clone(),
            });
            if let Err(Refusal::Failed(why)) = done {
                return Err(format!("{ident}: {why}"));
            }
        }
        Ok(())
    }

    async fn note(&self, change: Change) {
        let Some(path) = self.pending_path() else {
            return;
        };
        let shown = path.display().to_string();
        let done = tokio::task::spawn_blocking(move || match change {
            Change::Add(left) => pending::add(&path, &left),
            Change::Remove(fqdn, value) => pending::remove(&path, &fqdn, &value),
        })
        .await
        .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())));
        if let Err(e) = done {
            warn!("the list of DNS records to remove could not be saved in {shown}: {e}");
        }
    }

    async fn not_writable(
        &self,
        entry: usize,
        ident: &str,
        start: &Name,
        fqdn: &Name,
        why: &str,
    ) -> String {
        let label = self.entries[entry].provider.label();
        if start == fqdn {
            return format!(
                "{ident}: dns[{entry}] ({label}) cannot update {}: {why}",
                shown(fqdn)
            );
        }
        let target = shown(fqdn);
        let mut text = format!(
            "{} is a CNAME to {target}, which dns[{entry}] ({label}) cannot update: list {} on the entry that manages {}",
            shown(start),
            parent(ident),
            parent(&target)
        );
        if let Some(hint) = self.wildcard_cname(entry, start).await {
            text.push_str(&format!("; {hint}"));
        }
        text
    }

    async fn wildcard_cname(&self, entry: usize, start: &Name) -> Option<String> {
        let owner = start.base_name();
        let probe = name(&format!("_burrow-probe-{:x}.{}", now_secs(), shown(&owner))).ok()?;
        let lookup = self.lookup(entry);
        let first = |m: hickory_proto::op::Message, at: &Name| {
            m.answers.iter().find_map(|r| match &r.data {
                RData::CNAME(c) if r.name == *at => Some(c.0.clone()),
                _ => None,
            })
        };
        let direct = first(
            lookup.recursive(start, RecordType::CNAME).await.ok()?,
            start,
        )?;
        let probed = first(
            lookup.recursive(&probe, RecordType::CNAME).await.ok()?,
            &probe,
        )?;
        (direct == probed).then(|| {
            format!(
                "{} is answered by a wildcard CNAME (*.{}); add an explicit _acme-challenge record or remove the wildcard",
                shown(start),
                shown(&owner)
            )
        })
    }

    pub async fn wait(&self, set: &[Set]) -> Result<(), String> {
        let mut entries: Vec<usize> = set.iter().map(|s| s.entry).collect();
        entries.sort_unstable();
        entries.dedup();
        for entry in entries {
            let records: Vec<(Name, String)> = set
                .iter()
                .filter(|s| s.entry == entry)
                .map(|s| (s.fqdn.clone(), s.value.clone()))
                .collect();
            let limit = self.entries[entry].wait;
            super::propagate::wait_all(&self.lookup(entry), &records, limit, self.every).await?;
        }
        Ok(())
    }

    pub async fn cleanup(&self, set: &[Set]) {
        for s in set {
            let Ok(p) = self.provider(s.entry) else {
                continue;
            };
            match p.cleanup(&s.fqdn, &s.value).await {
                Ok(()) => {
                    self.note(Change::Remove(s.fqdn.to_ascii(), s.value.clone()))
                        .await;
                }
                Err(e) => warn!(
                    "the TXT record {} for {} could not be removed: {}; burrow tries again later",
                    shown(&s.fqdn),
                    s.name,
                    e.text()
                ),
            }
        }
    }

    pub async fn replay(&self) {
        let Some(path) = self.pending_path() else {
            return;
        };
        let p = path.clone();
        let left = tokio::task::spawn_blocking(move || pending::load(&p))
            .await
            .unwrap_or_default();
        let now = now_secs();
        for l in left {
            let found = entry_for(&self.entries, &l.name).map(|(i, _)| i);
            let old = pending::stale(std::slice::from_ref(&l), now).len() == 1;
            let (Some(entry), Ok(fqdn), false) = (found, name(&l.fqdn), old) else {
                warn!(
                    "the TXT record {} for {} was left from an earlier order and is no longer removed by burrow; remove it by hand",
                    l.fqdn, l.name
                );
                self.note(Change::Remove(l.fqdn.clone(), l.value.clone()))
                    .await;
                continue;
            };
            self.cleanup(&[Set {
                name: l.name.clone(),
                entry,
                fqdn,
                value: l.value.clone(),
            }])
            .await;
        }
    }
}

#[cfg(test)]
mod tests;
