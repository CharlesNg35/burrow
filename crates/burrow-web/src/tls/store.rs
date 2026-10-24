use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use burrow_syntax::web::{sni_name, wildcard_of};
use tokio_rustls::rustls::server::{ClientHello, ResolvesServerCert};
use tokio_rustls::rustls::sign::CertifiedKey;
use tracing::warn;

use super::files;
use super::select::{Choice, choose};
use super::{CertName, CertOrigin, CertPhase, CertState, CertStatus, Certificates};
use crate::CertSettings;

#[derive(Debug, Clone)]
struct Slot {
    key: Option<Arc<CertifiedKey>>,
    origin: Option<CertOrigin>,
    acme: bool,
    error: Option<String>,
}

#[derive(Debug, Default)]
struct Inner {
    slots: HashMap<String, Slot>,
    order: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Store {
    inner: RwLock<Inner>,
}

type Loaded = HashMap<(PathBuf, PathBuf), Result<Arc<CertifiedKey>, String>>;

impl Store {
    pub fn new() -> Arc<Store> {
        Arc::new(Store::default())
    }

    fn slot_for(&self, name: &str) -> Option<Slot> {
        let inner = self.inner.read().expect("certificate lock");
        let name = sni_name(name);
        inner
            .slots
            .get(&name)
            .or_else(|| wildcard_of(&name).and_then(|w| inner.slots.get(&w)))
            .cloned()
    }

    fn file_slot(
        name: &str,
        (cert, key, origin): (PathBuf, PathBuf, CertOrigin),
        loaded: &mut Loaded,
        old: Option<&Slot>,
    ) -> Slot {
        let got = loaded
            .entry((cert.clone(), key.clone()))
            .or_insert_with(|| files::load(&cert, &key))
            .clone();
        let fresh = got.and_then(|ck| {
            if files::covers(&ck, name) {
                Ok(ck)
            } else {
                Err(format!("{} does not cover \"{name}\"", cert.display()))
            }
        });
        match fresh {
            Ok(ck) => Slot {
                key: Some(ck),
                origin: Some(origin),
                acme: false,
                error: None,
            },
            Err(e) => {
                let kept = old.and_then(|o| o.key.clone());
                if kept.is_some() {
                    warn!("certificate for {name}: {e}; the previous certificate stays in use");
                } else {
                    warn!("certificate for {name}: {e}");
                }
                Slot {
                    key: kept,
                    origin: Some(origin),
                    acme: false,
                    error: Some(e),
                }
            }
        }
    }
}

impl ResolvesServerCert for Store {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.certificate(hello.server_name()?)
    }
}

impl Certificates for Store {
    fn state(&self, name: &str) -> CertState {
        match self.slot_for(name) {
            Some(Slot { key: Some(_), .. }) => CertState::Ready,
            Some(Slot { error: None, .. }) => CertState::Pending,
            _ => CertState::Failing,
        }
    }

    fn certificate(&self, name: &str) -> Option<Arc<CertifiedKey>> {
        self.slot_for(name)?.key
    }

    fn alpn_challenge(&self, _name: &str) -> Option<Arc<CertifiedKey>> {
        None
    }

    fn http_challenge(&self, _token: &str) -> Option<String> {
        None
    }

    fn apply(&self, settings: &CertSettings, names: &[CertName]) {
        let old = self.inner.read().expect("certificate lock").slots.clone();
        let mut loaded = Loaded::new();
        let mut next = Inner::default();
        for n in names {
            let slot = match choose(settings, n) {
                Choice::File { cert, key, origin } => {
                    Self::file_slot(&n.name, (cert, key, origin), &mut loaded, old.get(&n.name))
                }
                Choice::Acme => Slot {
                    key: old
                        .get(&n.name)
                        .filter(|o| o.acme)
                        .and_then(|o| o.key.clone()),
                    origin: None,
                    acme: true,
                    error: None,
                },
                Choice::Missing => Slot {
                    key: None,
                    origin: None,
                    acme: false,
                    error: Some(format!("\"{}\" has no certificate", n.name)),
                },
            };
            next.order.push(n.name.clone());
            next.slots.insert(n.name.clone(), slot);
        }
        *self.inner.write().expect("certificate lock") = next;
    }

    fn status(&self) -> Vec<CertStatus> {
        let inner = self.inner.read().expect("certificate lock");
        inner
            .order
            .iter()
            .filter_map(|name| {
                let s = inner.slots.get(name)?;
                let origin = s.origin.clone()?;
                let phase = match (&s.key, &s.error) {
                    (None, Some(e)) => CertPhase::Failing { reason: e.clone() },
                    (None, None) => CertPhase::Pending,
                    _ => CertPhase::Ok,
                };
                Some(CertStatus {
                    name: name.clone(),
                    origin,
                    check: None,
                    phase,
                    expires: None,
                    next_renewal: None,
                    last_error: s.error.clone(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
