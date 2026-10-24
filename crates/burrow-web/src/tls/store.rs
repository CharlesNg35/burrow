use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use burrow_syntax::web::{sni_name, wildcard_of};
use tokio_rustls::rustls::server::{ClientHello, ResolvesServerCert};
use tokio_rustls::rustls::sign::CertifiedKey;
use tracing::warn;

use super::select::{Choice, choose};
use super::{Applying, CertName, CertOrigin, CertPhase, CertState, CertStatus, Certificates};
use super::{files, x509};
use crate::CertSettings;

pub const EXPIRY_WARNING_SECS: u64 = 14 * 24 * 3600;

#[derive(Debug, Clone)]
struct Slot {
    key: Option<Arc<CertifiedKey>>,
    origin: Option<CertOrigin>,
    acme: bool,
    error: Option<String>,
    expires: Option<u64>,
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

type Pair = (PathBuf, PathBuf);
type Loaded = HashMap<Pair, Result<Arc<CertifiedKey>, String>>;

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn expires(ck: &CertifiedKey) -> Option<u64> {
    x509::facts(ck.cert.first()?).map(|f| f.not_after)
}

fn warn_expiry(name: &str, path: &std::path::Path, expires: Option<u64>) {
    let Some(at) = expires else {
        return;
    };
    let now = now_secs();
    if at <= now {
        warn!(
            "certificate for {name} ({}) has expired; replace the file",
            path.display()
        );
    } else if at - now < EXPIRY_WARNING_SECS {
        let days = (at - now) / 86_400;
        warn!(
            "certificate for {name} ({}) expires in {days} {}; replace the file",
            path.display(),
            if days == 1 { "day" } else { "days" }
        );
    }
}

async fn load_all(pairs: Vec<Pair>) -> Loaded {
    tokio::task::spawn_blocking(move || {
        pairs
            .into_iter()
            .map(|(c, k)| {
                let got = files::load(&c, &k);
                ((c, k), got)
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

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
        loaded: &Loaded,
        old: Option<&Slot>,
    ) -> Slot {
        let got = loaded
            .get(&(cert.clone(), key.clone()))
            .cloned()
            .unwrap_or_else(|| Err(format!("cannot read {}", cert.display())));
        let fresh = got.and_then(|ck| {
            if files::covers(&ck, name) {
                Ok(ck)
            } else {
                Err(format!("{} does not cover \"{name}\"", cert.display()))
            }
        });
        match fresh {
            Ok(ck) => {
                let at = expires(&ck);
                warn_expiry(name, &cert, at);
                Slot {
                    key: Some(ck),
                    origin: Some(origin),
                    acme: false,
                    error: None,
                    expires: at,
                }
            }
            Err(e) => {
                let kept = old.and_then(|o| o.key.clone());
                if kept.is_some() {
                    warn!("certificate for {name}: {e}; the previous certificate stays in use");
                } else {
                    warn!("certificate for {name}: {e}");
                }
                Slot {
                    expires: kept.as_deref().and_then(expires),
                    key: kept,
                    origin: Some(origin),
                    acme: false,
                    error: Some(e),
                }
            }
        }
    }
}

impl Store {
    fn install(&self, plan: Vec<(&CertName, Choice)>, loaded: &Loaded) {
        let mut inner = self.inner.write().expect("certificate lock");
        let mut next = Inner::default();
        for (n, choice) in plan {
            let old = inner.slots.get(&n.name);
            let slot = match choice {
                Choice::File { cert, key, origin } => {
                    Self::file_slot(&n.name, (cert, key, origin), loaded, old)
                }
                Choice::Acme => Slot {
                    key: old.filter(|o| o.acme).and_then(|o| o.key.clone()),
                    origin: None,
                    acme: true,
                    error: None,
                    expires: old.filter(|o| o.acme).and_then(|o| o.expires),
                },
                Choice::Missing => Slot {
                    key: None,
                    origin: None,
                    acme: false,
                    error: Some(format!("\"{}\" has no certificate", n.name)),
                    expires: None,
                },
            };
            next.order.push(n.name.clone());
            next.slots.insert(n.name.clone(), slot);
        }
        *inner = next;
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

    fn apply<'a>(&'a self, settings: &'a CertSettings, names: &'a [CertName]) -> Applying<'a> {
        Box::pin(async move {
            let plan: Vec<(&CertName, Choice)> =
                names.iter().map(|n| (n, choose(settings, n))).collect();
            let mut pairs: Vec<Pair> = plan
                .iter()
                .filter_map(|(_, c)| match c {
                    Choice::File { cert, key, .. } => Some((cert.clone(), key.clone())),
                    _ => None,
                })
                .collect();
            pairs.sort();
            pairs.dedup();
            let loaded = load_all(pairs).await;
            self.install(plan, &loaded);
        })
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
                    expires: s.expires,
                    next_renewal: None,
                    last_error: s.error.clone(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
