use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use burrow_syntax::web::{sni_name, wildcard_of};
use tokio_rustls::rustls::server::{ClientHello, ResolvesServerCert};
use tokio_rustls::rustls::sign::CertifiedKey;
use tracing::warn;

use super::acme::{Acme, Handle, Plan};
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
    group: String,
    error: Option<String>,
    expires: Option<u64>,
}

#[derive(Debug, Default)]
struct Inner {
    slots: HashMap<String, Slot>,
    order: Vec<String>,
    directory: String,
}

#[derive(Default)]
pub struct Store {
    inner: RwLock<Inner>,
    acme: Arc<Acme>,
    manager: Mutex<Option<Handle>>,
    warned: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
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

fn warn_expiry(name: &str, path: &std::path::Path, expires: Option<u64>) -> bool {
    let Some(at) = expires else {
        return false;
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
    } else {
        return false;
    }
    true
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

    fn slot_for(&self, name: &str) -> Option<(String, Slot)> {
        let inner = self.inner.read().expect("certificate lock");
        let name = sni_name(name);
        let key = if inner.slots.contains_key(&name) {
            name
        } else {
            wildcard_of(&name).filter(|w| inner.slots.contains_key(w))?
        };
        inner.slots.get(&key).cloned().map(|s| (key, s))
    }

    async fn run_acme(&self, settings: &CertSettings) {
        let groups: BTreeMap<String, Vec<String>> = {
            let inner = self.inner.read().expect("certificate lock");
            let mut g: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for n in &inner.order {
                if let Some(s) = inner.slots.get(n).filter(|s| s.acme) {
                    g.entry(s.group.clone()).or_default().push(n.clone());
                }
            }
            g
        };
        let plan = settings
            .acme
            .clone()
            .filter(|_| !groups.is_empty())
            .map(|settings| Plan { settings, groups });
        let Some(plan) = plan else {
            let stopped = self.manager.lock().expect("manager lock").take();
            drop(stopped);
            self.acme.clear();
            return;
        };
        self.acme.preload(&plan).await;
        let mut manager = self.manager.lock().expect("manager lock");
        match manager.as_ref() {
            Some(h) => h.update(plan),
            None => *manager = Some(Handle::start(self.acme.clone(), plan)),
        }
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
                    group: String::new(),
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
                    group: String::new(),
                    error: Some(e),
                }
            }
        }
    }
}

impl Store {
    fn install(&self, settings: &CertSettings, plan: Vec<(&CertName, Choice)>, loaded: &Loaded) {
        let mut inner = self.inner.write().expect("certificate lock");
        let mut next = Inner {
            directory: settings
                .acme
                .as_ref()
                .map(|a| a.directory_url.clone())
                .unwrap_or_default(),
            ..Inner::default()
        };
        for (n, choice) in plan {
            let old = inner.slots.get(&n.name);
            let slot = match choice {
                Choice::File { cert, key, origin } => {
                    Self::file_slot(&n.name, (cert, key, origin), loaded, old)
                }
                Choice::Acme => Slot {
                    key: None,
                    origin: None,
                    acme: true,
                    group: n.group.clone(),
                    error: None,
                    expires: None,
                },
                Choice::Missing(why) => Slot {
                    key: None,
                    origin: None,
                    acme: false,
                    group: n.group.clone(),
                    error: Some(why),
                    expires: None,
                },
            };
            next.order.push(n.name.clone());
            next.slots.insert(n.name.clone(), slot);
        }
        *inner = next;
    }
}

impl Store {
    fn acme_status(&self, name: &str, s: &Slot, directory: &str) -> Option<CertStatus> {
        let view = self.acme.view_of(&s.group);
        let served = self.acme.certificate(name);
        Some(CertStatus {
            name: name.to_string(),
            origin: CertOrigin::LetsEncrypt {
                directory: directory.to_string(),
            },
            check: view.as_ref().and_then(|v| v.check),
            phase: view
                .as_ref()
                .map_or(CertPhase::Pending, |v| v.phase.clone()),
            expires: served.as_deref().and_then(expires),
            next_renewal: view.as_ref().and_then(|v| v.next_renewal),
            last_error: view.as_ref().and_then(|v| v.last_error.clone()),
            explanation: view.as_ref().and_then(|v| v.explanation.clone()),
            failures: view.map_or(0, |v| v.failures),
        })
    }
}

impl Store {
    pub fn warn_expiring(&self) -> usize {
        use std::sync::atomic::Ordering;
        let now = now_secs();
        let last = self.warned.load(Ordering::Relaxed);
        if now < last + 24 * 3600
            || self
                .warned
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            return 0;
        }
        let inner = self.inner.read().expect("certificate lock");
        inner
            .order
            .iter()
            .filter(|name| {
                inner.slots.get(*name).is_some_and(|s| match &s.origin {
                    Some(
                        CertOrigin::OwnFile { path }
                        | CertOrigin::SharedFile { path }
                        | CertOrigin::TunnelFile { path },
                    ) => warn_expiry(name, path, s.expires),
                    _ => false,
                })
            })
            .count()
    }
}

impl ResolvesServerCert for Store {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.certificate(hello.server_name()?)
    }
}

impl Certificates for Store {
    fn state(&self, name: &str) -> CertState {
        let Some((configured, slot)) = self.slot_for(name) else {
            return CertState::Failing;
        };
        if slot.acme {
            if self.acme.certificate(&configured).is_some() {
                return CertState::Ready;
            }
            return match self.acme.view_of(&slot.group).map(|v| v.phase) {
                Some(CertPhase::Failing { .. }) => CertState::Failing,
                _ => CertState::Pending,
            };
        }
        match slot {
            Slot { key: Some(_), .. } => CertState::Ready,
            Slot { error: None, .. } => CertState::Pending,
            _ => CertState::Failing,
        }
    }

    fn certificate(&self, name: &str) -> Option<Arc<CertifiedKey>> {
        let (configured, slot) = self.slot_for(name)?;
        if slot.acme {
            self.acme.certificate(&configured)
        } else {
            slot.key
        }
    }

    fn alpn_challenge(&self, name: &str) -> Option<Arc<CertifiedKey>> {
        self.acme.challenges.alpn(name)
    }

    fn http_challenge(&self, token: &str) -> Option<String> {
        self.acme.challenges.http(token)
    }

    fn account(&self) -> Option<String> {
        self.acme.account()
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
            self.install(settings, plan, &loaded);
            self.run_acme(settings).await;
        })
    }

    fn status(&self) -> Vec<CertStatus> {
        let inner = self.inner.read().expect("certificate lock");
        inner
            .order
            .iter()
            .filter_map(|name| {
                let s = inner.slots.get(name)?;
                if s.acme {
                    return self.acme_status(name, s, &inner.directory);
                }
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
                    explanation: None,
                    failures: 0,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
