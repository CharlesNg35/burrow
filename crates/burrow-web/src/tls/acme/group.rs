use std::path::Path;
use std::sync::Arc;

use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::sign::CertifiedKey;

use super::manager::GroupView;
use super::{schedule, storage};
use crate::tls::store::now_secs;
use crate::tls::{CertCheck, CertPhase, files, x509};

#[derive(Debug, Clone)]
pub struct Held {
    pub key: Arc<CertifiedKey>,
    pub names: Vec<String>,
    pub not_before: u64,
    pub not_after: u64,
    pub ari_id: Option<String>,
}

impl Held {
    pub fn parse(chain_pem: &str, key_pem: &str, names: &[String]) -> Result<Held, String> {
        let chain = CertificateDer::pem_slice_iter(chain_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("the CA sent an unreadable chain: {e}"))?;
        let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
            .map_err(|e| format!("the new key is unreadable: {e}"))?;
        let key = Arc::new(files::certified(chain, key)?);
        if let Some(n) = names.iter().find(|n| !files::covers(&key, n)) {
            return Err(format!("the new certificate does not cover \"{n}\""));
        }
        let facts = x509::facts(&key.cert[0]).ok_or("the new certificate has no readable dates")?;
        Ok(Held {
            key,
            names: names.to_vec(),
            not_before: facts.not_before,
            not_after: facts.not_after,
            ari_id: facts.ari_id,
        })
    }

    pub fn covers(&self, names: &[String]) -> bool {
        names.iter().all(|n| files::covers(&self.key, n))
    }

    pub fn find(state: &Path, directory: &str, names: &[String], now: u64) -> Option<Held> {
        Held::best(state, directory, names, |h| h.not_after > now)
    }

    pub fn reusable(
        state: &Path,
        directory: &str,
        names: &[String],
        now: u64,
        not: Option<&Held>,
    ) -> Option<Held> {
        Held::best(state, directory, names, |h| {
            now < schedule::renew_at(h.not_before, h.not_after)
                && not.is_none_or(|n| n.key.cert[0] != h.key.cert[0])
        })
    }

    fn best(
        state: &Path,
        directory: &str,
        names: &[String],
        keep: impl Fn(&Held) -> bool,
    ) -> Option<Held> {
        let wanted = storage::name_set(names);
        storage::all(state)
            .into_iter()
            .filter(|(_, s)| s.meta.directory == directory)
            .filter_map(|(_, s)| Held::parse(&s.chain_pem, &s.key_pem, &s.meta.names).ok())
            .filter(|h| h.covers(names) && keep(h))
            .max_by_key(|h| (storage::name_set(&h.names) == wanted, h.not_after))
    }

    pub(super) fn short_lived(&self) -> bool {
        schedule::short_lived(self.not_before, self.not_after)
    }
}

#[derive(Debug)]
pub struct Group {
    pub(super) names: Vec<String>,
    pub(super) held: Option<Held>,
    pub(super) next: u64,
    pub(super) planned: Option<u64>,
    pub(super) failures: u32,
    pub(super) total: u64,
    pub(super) last_error: Option<String>,
    pub(super) code: Option<&'static str>,
    pub(super) explanation: Option<String>,
    pub(super) check: Option<CertCheck>,
    pub(super) staging_at: Option<u64>,
    pub(super) prod_failed_at: Option<u64>,
    pub(super) fallback_note: Option<String>,
    pub(super) replaces_refused: u8,
    pub(super) running: bool,
}

impl Group {
    pub fn new(names: Vec<String>, held: Option<Held>) -> Group {
        Group {
            names,
            held,
            next: now_secs(),
            planned: None,
            failures: 0,
            total: 0,
            last_error: None,
            code: None,
            explanation: None,
            check: None,
            staging_at: None,
            prod_failed_at: None,
            fallback_note: None,
            replaces_refused: 0,
            running: false,
        }
    }

    pub fn rename(&mut self, names: &[String]) {
        if self.names != names {
            self.names = names.to_vec();
            self.next = now_secs();
            self.failures = 0;
        }
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn in_use(&self) -> Vec<Vec<String>> {
        std::iter::once(self.names.clone())
            .chain(self.held.as_ref().map(|h| h.names.clone()))
            .collect()
    }

    pub fn due(&self) -> u64 {
        self.staging_at.map_or(self.next, |s| s.min(self.next))
    }

    pub(super) fn needs_order(&self, now: u64) -> bool {
        match &self.held {
            None => true,
            Some(h) => {
                !h.covers(&self.names)
                    || h.not_after <= now
                    || self.planned.is_some_and(|p| p <= now)
            }
        }
    }

    pub fn view(&self, checks: &[CertCheck]) -> GroupView {
        let reason = || self.last_error.clone().unwrap_or_default();
        let phase = match (&self.held, &self.last_error, self.running) {
            (_, _, true) if self.held.is_some() => CertPhase::Renewing,
            (None, None, _) => CertPhase::Pending,
            (_, Some(_), _) => CertPhase::Failing { reason: reason() },
            (Some(_), None, _) => CertPhase::Ok,
        };
        let last_error = match (&self.last_error, &self.fallback_note) {
            (Some(e), Some(n)) => Some(format!("{e}; {n}")),
            (e, _) => e.clone(),
        };
        GroupView {
            names: self.names.clone(),
            check: self.check.or_else(|| checks.first().copied()),
            phase,
            expires: self.held.as_ref().map(|h| h.not_after),
            next_renewal: Some(match &self.held {
                Some(h) if self.last_error.is_none() => self
                    .planned
                    .unwrap_or_else(|| schedule::renew_at(h.not_before, h.not_after)),
                _ => self.next,
            }),
            last_error,
            code: self.last_error.as_ref().and(self.code),
            explanation: self.explanation.clone(),
            failures: self.total,
            queued: (self.running || self.needs_order(now_secs())).then_some(self.next),
            running: self.running,
        }
    }
}
