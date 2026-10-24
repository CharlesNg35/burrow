use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::sign::CertifiedKey;

use super::challenge::Challenges;
use super::job::{Group, Held, Worker};
use super::storage;
use crate::AcmeSettings;
use crate::tls::store::now_secs;
use crate::tls::{CertCheck, CertPhase};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub settings: AcmeSettings,
    pub groups: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupView {
    pub names: Vec<String>,
    pub check: Option<CertCheck>,
    pub phase: CertPhase,
    pub expires: Option<u64>,
    pub next_renewal: Option<u64>,
    pub last_error: Option<String>,
    pub explanation: Option<String>,
    pub failures: u64,
}

#[derive(Debug, Default)]
pub struct Acme {
    pub challenges: Challenges,
    certs: RwLock<HashMap<String, Arc<CertifiedKey>>>,
    views: RwLock<BTreeMap<String, GroupView>>,
    account: RwLock<Option<String>>,
}

impl Acme {
    pub fn certificate(&self, name: &str) -> Option<Arc<CertifiedKey>> {
        self.certs.read().expect("acme lock").get(name).cloned()
    }

    pub fn view_of(&self, group: &str) -> Option<GroupView> {
        self.views.read().expect("acme lock").get(group).cloned()
    }

    pub fn account(&self) -> Option<String> {
        self.account.read().expect("acme lock").clone()
    }

    pub(super) fn set_account(&self, id: Option<String>) {
        *self.account.write().expect("acme lock") = id;
    }

    pub(super) fn serve(&self, names: &[String], key: &Arc<CertifiedKey>) {
        let mut certs = self.certs.write().expect("acme lock");
        for n in names {
            if crate::tls::files::covers(key, n) {
                certs.insert(n.clone(), key.clone());
            }
        }
    }

    pub(super) fn publish(&self, group: &str, view: GroupView) {
        self.views
            .write()
            .expect("acme lock")
            .insert(group.to_string(), view);
    }

    pub fn clear(&self) {
        self.certs.write().expect("acme lock").clear();
        self.views.write().expect("acme lock").clear();
        self.set_account(None);
    }

    fn keep_only(&self, plan: &Plan) {
        let names: Vec<&String> = plan.groups.values().flatten().collect();
        self.certs
            .write()
            .expect("acme lock")
            .retain(|n, _| names.contains(&n));
        self.views
            .write()
            .expect("acme lock")
            .retain(|g, _| plan.groups.contains_key(g));
    }

    pub async fn preload(&self, plan: &Plan) {
        let Some(state) = plan.settings.state_dir.clone() else {
            return;
        };
        let missing: Vec<(String, Vec<String>)> = plan
            .groups
            .iter()
            .filter(|(_, names)| names.iter().any(|n| self.certificate(n).is_none()))
            .map(|(g, n)| (g.clone(), n.clone()))
            .collect();
        let directory = plan.settings.directory_url.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            missing
                .into_iter()
                .filter_map(|(g, names)| {
                    let held = Held::stored(&state, &g, &directory)?;
                    Some((names, held))
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        for (names, held) in loaded {
            self.serve(&names, &held.key);
        }
    }
}

pub struct Handle {
    tx: watch::Sender<Plan>,
    task: JoinHandle<()>,
}

impl Handle {
    pub fn start(acme: Arc<Acme>, plan: Plan) -> Handle {
        let (tx, rx) = watch::channel(plan);
        let task = tokio::spawn(run(acme, rx));
        Handle { tx, task }
    }

    pub fn update(&self, plan: Plan) {
        self.tx.send_if_modified(|p| {
            let changed = *p != plan;
            *p = plan;
            changed
        });
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

const SWEEP_EVERY: u64 = 24 * 3600;

async fn run(acme: Arc<Acme>, mut rx: watch::Receiver<Plan>) {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut worker = Worker::new(acme.clone());
    let mut swept = 0;
    loop {
        let plan = rx.borrow_and_update().clone();
        acme.keep_only(&plan);
        worker.adopt(&plan);
        sync(&mut groups, &plan).await;
        for (name, g) in &groups {
            acme.publish(name, g.view(worker.checks()));
        }
        if now_secs() >= swept + SWEEP_EVERY {
            swept = now_secs();
            sweep(&plan, swept).await;
        }
        let due = groups.iter().map(|(name, g)| (g.due(), name.clone())).min();
        let wait = due
            .as_ref()
            .map_or(Duration::from_secs(SWEEP_EVERY), |(at, _)| {
                Duration::from_secs(at.saturating_sub(now_secs()))
            });
        tokio::select! {
            _ = tokio::time::sleep(wait) => {
                if let Some((_, name)) = due
                    && let Some(g) = groups.get_mut(&name)
                {
                    worker.step(&name, g).await;
                    acme.publish(&name, g.view(worker.checks()));
                }
            }
            changed = rx.changed() => {
                if changed.is_err() {
                    return;
                }
                worker.forget_accounts();
            }
        }
    }
}

async fn sync(groups: &mut BTreeMap<String, Group>, plan: &Plan) {
    groups.retain(|g, _| plan.groups.contains_key(g));
    let state = plan.settings.state_dir.clone();
    let directory = plan.settings.directory_url.clone();
    for (name, names) in &plan.groups {
        match groups.get_mut(name) {
            Some(g) => g.rename(names),
            None => {
                let stored = match &state {
                    Some(s) => {
                        let (s, n, d) = (s.clone(), name.clone(), directory.clone());
                        tokio::task::spawn_blocking(move || Held::stored(&s, &n, &d))
                            .await
                            .ok()
                            .flatten()
                    }
                    None => None,
                };
                groups.insert(name.clone(), Group::new(names.clone(), stored));
            }
        }
    }
}

async fn sweep(plan: &Plan, now: u64) {
    let Some(state) = plan.settings.state_dir.clone() else {
        return;
    };
    let kept: Vec<PathBuf> = plan
        .groups
        .keys()
        .map(|g| storage::group_dir(&state, g))
        .collect();
    let _ = tokio::task::spawn_blocking(move || storage::sweep(&state, &kept, now)).await;
}
