mod ca;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use tokio_rustls::rustls::sign::CertifiedKey;
use tracing::{info, warn};

pub use ca::{
    Ca, INTERMEDIATE_SECS, LEAF_SECS, PERMITTED_NAMES, PERMITTED_NETS, ROOT_SECS,
    ROOT_WARNING_SECS, SKEW_SECS, due, fingerprint, leaf_due, leaf_renew_at,
};

pub const CHECK_EVERY_SECS: u64 = 600;

pub type Write<'a> = &'a dyn Fn(&Path, &[u8]) -> std::io::Result<()>;
pub type Writer = crate::tls::acme::storage::Writer;

pub fn default_writer() -> Writer {
    Arc::new(|p: &Path, b: &[u8]| crate::tls::acme::storage::write_private(p, b))
}

pub fn dir(state: &Path) -> PathBuf {
    state.join("ca")
}

pub fn root_pem_path(state: &Path) -> PathBuf {
    dir(state).join("root.pem")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCa {
    pub subject: String,
    pub sha256: String,
    pub not_before: u64,
    pub not_after: u64,
    pub root_path: PathBuf,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Leaf {
    pub key: Arc<CertifiedKey>,
    pub names: Vec<String>,
    pub not_before: u64,
    pub not_after: u64,
}

type Wanted = (Option<PathBuf>, BTreeMap<String, Vec<String>>);

#[derive(Default)]
pub struct Internal {
    ca: Mutex<Option<Ca>>,
    leaves: RwLock<HashMap<String, Leaf>>,
    wanted: RwLock<Wanted>,
    error: RwLock<Option<String>>,
}

impl Internal {
    pub fn plan(&self, state: Option<PathBuf>, groups: BTreeMap<String, Vec<String>>) {
        *self.wanted.write().expect("internal lock") = (state, groups);
    }

    pub fn active(&self) -> bool {
        !self.wanted.read().expect("internal lock").1.is_empty()
    }

    pub fn leaf(&self, group: &str) -> Option<Leaf> {
        self.leaves
            .read()
            .expect("internal lock")
            .get(group)
            .cloned()
    }

    pub fn error(&self) -> Option<String> {
        self.error.read().expect("internal lock").clone()
    }

    fn fail(&self, why: String) {
        warn!("local CA: {why}");
        *self.error.write().expect("internal lock") = Some(why);
    }

    pub fn step(&self, now: u64, write: Write<'_>) {
        let (state, groups) = self.wanted.read().expect("internal lock").clone();
        self.leaves
            .write()
            .expect("internal lock")
            .retain(|g, _| groups.contains_key(g));
        if groups.is_empty() {
            return;
        }
        let Some(state) = state else {
            self.fail("burrow has no state folder for its local CA".into());
            return;
        };
        let mut held = self.ca.lock().expect("internal lock");
        if held.is_none() {
            let folder = dir(&state);
            let opened = if Ca::exists(&folder) {
                Ca::open(&folder, now, write)
            } else {
                Ca::create(&folder, now, write).inspect(|ca| {
                    info!(
                        "local CA \"{}\" created in {}; install {} on your devices to trust it",
                        ca.subject(),
                        folder.display(),
                        root_pem_path(&state).display()
                    );
                })
            };
            match opened {
                Ok(ca) => *held = Some(ca),
                Err(e) => return self.fail(e),
            }
        }
        let Some(ca) = held.as_mut() else {
            return;
        };
        let renewed = match ca.refresh(now, write) {
            Ok(r) => r,
            Err(e) => return self.fail(e),
        };
        let mut errors = Vec::new();
        for (group, names) in &groups {
            let current = self.leaf(group);
            let fresh = current.as_ref().is_some_and(|l| {
                l.names == *names && !leaf_due(l.not_before, l.not_after, now) && !renewed
            });
            if fresh {
                continue;
            }
            match ca.issue(names, now) {
                Ok((key, not_after)) => {
                    let leaf = Leaf {
                        key,
                        names: names.clone(),
                        not_before: now.saturating_sub(SKEW_SECS),
                        not_after,
                    };
                    self.leaves
                        .write()
                        .expect("internal lock")
                        .insert(group.clone(), leaf);
                }
                Err(e) => errors.push(format!("{}: {e}", names.join(", "))),
            }
        }
        *self.error.write().expect("internal lock") =
            (!errors.is_empty()).then(|| errors.join("; "));
    }

    pub fn status(&self, now: u64) -> Option<LocalCa> {
        let state = self.wanted.read().expect("internal lock").0.clone()?;
        let held = self.ca.lock().expect("internal lock");
        let ca = held.as_ref()?;
        let left = ca.root_not_after.saturating_sub(now);
        Some(LocalCa {
            subject: ca.subject().to_string(),
            sha256: fingerprint(&ca.root),
            not_before: ca.root_not_before,
            not_after: ca.root_not_after,
            root_path: root_pem_path(&state),
            warning: (left < ROOT_WARNING_SECS).then(|| {
                format!(
                    "the local CA expires in {} days; burrow does not renew it on its own",
                    left / 86_400
                )
            }),
        })
    }
}

#[cfg(test)]
mod tests;
