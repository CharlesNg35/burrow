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

#[derive(Debug, Clone)]
struct Slot {
    key: Option<Arc<CertifiedKey>>,
    origin: Option<CertOrigin>,
    acme: bool,
    internal: bool,
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
    local: Arc<super::internal::Internal>,
    local_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

enum Serve {
    File {
        key: Option<Arc<CertifiedKey>>,
        failed: bool,
    },
    Acme {
        name: String,
        group: String,
        failed: bool,
    },
    Internal(String),
}

type Pair = (PathBuf, PathBuf);
type Loaded = HashMap<Pair, Result<Arc<CertifiedKey>, String>>;

use burrow_syntax::duration::DAY_SECS;
pub use burrow_syntax::duration::now_secs;

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
    } else if burrow_syntax::web::expires_soon(at, now) {
        warn!(
            "certificate for {name} ({}) expires in {}; replace the file",
            path.display(),
            burrow_syntax::duration::days_left(at, now)
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

    #[cfg(any(test, feature = "admin-test-hooks"))]
    pub fn set_dns_reach(&self, reach: super::acme::dns01::wire::Reach) {
        self.acme.set_dns_reach(reach);
    }

    fn slot_for(&self, name: &str) -> Option<Serve> {
        let inner = self.inner.read().expect("certificate lock");
        let name = sni_name(name);
        let key = if inner.slots.contains_key(&name) {
            name
        } else {
            wildcard_of(&name).filter(|w| inner.slots.contains_key(w))?
        };
        let slot = inner.slots.get(&key)?;
        Some(if slot.internal {
            Serve::Internal(slot.group.clone())
        } else if slot.acme {
            Serve::Acme {
                name: key,
                group: slot.group.clone(),
                failed: slot.error.is_some(),
            }
        } else {
            Serve::File {
                key: slot.key.clone(),
                failed: slot.error.is_some(),
            }
        })
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
            .map(|acme| Plan {
                settings: acme,
                groups,
                choices: settings.choices.clone(),
            });
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
                    internal: false,
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
                    internal: false,
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
                Choice::Internal => Slot {
                    key: None,
                    origin: Some(CertOrigin::Internal),
                    acme: false,
                    internal: true,
                    group: n.group.clone(),
                    error: None,
                    expires: None,
                },
                Choice::Acme => Slot {
                    key: None,
                    origin: None,
                    acme: true,
                    internal: false,
                    group: n.group.clone(),
                    error: settings.acme.as_ref().and_then(|a| {
                        super::acme::checks_of(a, &n.name, settings.choices.get(&n.group).copied())
                            .err()
                    }),
                    expires: None,
                },
                Choice::Missing(why) => Slot {
                    key: None,
                    origin: None,
                    acme: false,
                    internal: false,
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
            phase: match view.as_ref().map(|v| &v.phase) {
                _ if served.is_none() && s.error.is_some() => CertPhase::Failing {
                    reason: s.error.clone().unwrap_or_default(),
                },
                Some(CertPhase::Failing { reason }) => CertPhase::Failing {
                    reason: reason.clone(),
                },
                Some(p) if served.is_some() => p.clone(),
                None if served.is_some() => CertPhase::Ok,
                _ => CertPhase::Pending,
            },
            expires: served.as_deref().and_then(expires),
            next_renewal: view.as_ref().and_then(|v| v.next_renewal),
            last_error: match (&s.error, &served) {
                (Some(e), None) => Some(e.clone()),
                _ => view.as_ref().and_then(|v| v.last_error.clone()),
            },
            code: view.as_ref().and_then(|v| v.code),
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
        if now < last + DAY_SECS
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
        match self.slot_for(name) {
            None => CertState::Failing,
            Some(Serve::Internal(group)) => {
                match (self.local.leaf_key(&group), self.local.error()) {
                    (Some(_), _) => CertState::Ready,
                    (None, Some(_)) => CertState::Failing,
                    (None, None) => CertState::Pending,
                }
            }
            Some(Serve::Acme {
                name,
                group,
                failed,
            }) => {
                if self.acme.certificate(&name).is_some() {
                    return CertState::Ready;
                }
                if failed {
                    return CertState::Failing;
                }
                match self.acme.view_of(&group).map(|v| v.phase) {
                    Some(CertPhase::Failing { .. }) => CertState::Failing,
                    _ => CertState::Pending,
                }
            }
            Some(Serve::File { key: Some(_), .. }) => CertState::Ready,
            Some(Serve::File { failed: false, .. }) => CertState::Pending,
            Some(Serve::File { .. }) => CertState::Failing,
        }
    }

    fn certificate(&self, name: &str) -> Option<Arc<CertifiedKey>> {
        match self.slot_for(name)? {
            Serve::Internal(group) => self.local.leaf_key(&group),
            Serve::Acme { name, .. } => self.acme.certificate(&name),
            Serve::File { key, .. } => key,
        }
    }

    fn alpn_challenge(&self, name: &str) -> Option<Arc<CertifiedKey>> {
        self.acme.challenges.alpn(name)
    }

    fn http_challenge(&self, token: &str) -> Option<String> {
        self.acme.challenges.http(token)
    }

    fn local_ca(&self) -> Option<super::internal::LocalCa> {
        self.local.status(now_secs())
    }

    fn account(&self) -> Option<String> {
        self.acme.account()
    }

    fn queue(&self) -> Vec<super::CertOrder> {
        self.acme.queue()
    }

    fn apply<'a>(&'a self, settings: &'a CertSettings, names: &'a [CertName]) -> Applying<'a> {
        Box::pin(async move {
            let settings = &super::select::wildcard_checks(settings, names);
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
            self.run_local(settings).await;
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
                if s.internal {
                    return Some(self.local_status(name, s));
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
                    code: None,
                    explanation: None,
                    failures: 0,
                })
            })
            .collect()
    }
}

mod local;

#[cfg(test)]
mod tests;
