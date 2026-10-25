use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::sign::CertifiedKey;

use super::challenge::Challenges;
use super::dns01::wire::Reach;
use super::job::{Group, Held, Worker};
use super::storage;
use crate::AcmeSettings;
use crate::tls::store::now_secs;
use crate::tls::{CertCheck, CertPhase};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub settings: AcmeSettings,
    pub groups: BTreeMap<String, Vec<String>>,
    pub choices: BTreeMap<String, burrow_syntax::web::Challenge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupView {
    pub names: Vec<String>,
    pub check: Option<CertCheck>,
    pub phase: CertPhase,
    pub expires: Option<u64>,
    pub next_renewal: Option<u64>,
    pub last_error: Option<String>,
    pub code: Option<&'static str>,
    pub explanation: Option<String>,
    pub failures: u64,
    pub queued: Option<u64>,
    pub running: bool,
}

#[derive(Debug, Default)]
pub struct Acme {
    pub challenges: Challenges,
    certs: RwLock<HashMap<String, Arc<CertifiedKey>>>,
    views: RwLock<BTreeMap<String, GroupView>>,
    account: RwLock<Option<String>>,
    reach: RwLock<Reach>,
}

impl Acme {
    pub fn certificate(&self, name: &str) -> Option<Arc<CertifiedKey>> {
        self.certs.read().expect("acme lock").get(name).cloned()
    }

    pub fn view_of(&self, group: &str) -> Option<GroupView> {
        self.views.read().expect("acme lock").get(group).cloned()
    }

    pub fn queue(&self) -> Vec<crate::tls::CertOrder> {
        let mut q: Vec<crate::tls::CertOrder> = self
            .views
            .read()
            .expect("acme lock")
            .iter()
            .filter_map(|(group, v)| {
                Some(crate::tls::CertOrder {
                    service: group.clone(),
                    names: v.names.clone(),
                    at: v.queued?,
                    running: v.running,
                })
            })
            .collect();
        q.sort_by_key(|o| (!o.running, o.at));
        q
    }

    pub(super) fn dns_reach(&self) -> Reach {
        self.reach.read().expect("acme lock").clone()
    }

    #[cfg(any(test, feature = "admin-test-hooks"))]
    pub fn set_dns_reach(&self, reach: Reach) {
        *self.reach.write().expect("acme lock") = reach;
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
        let wanted = orderable(plan);
        self.views
            .write()
            .expect("acme lock")
            .retain(|g, _| wanted.contains_key(g));
    }

    pub async fn preload(&self, plan: &Plan) {
        let Some(state) = plan.settings.state_dir.clone() else {
            return;
        };
        let mut missing: Vec<Vec<String>> = plan
            .groups
            .values()
            .filter(|names| names.iter().any(|n| self.certificate(n).is_none()))
            .cloned()
            .collect();
        missing.extend(
            plan.groups
                .values()
                .flatten()
                .filter(|n| self.certificate(n).is_none())
                .map(|n| vec![n.clone()]),
        );
        let directory = plan.settings.directory_url.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            missing
                .into_iter()
                .filter_map(|names| {
                    let held = Held::find(&state, &directory, &names, now_secs())?;
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
        worker.replay().await;
        sync(&mut groups, &plan).await;
        for (name, g) in &groups {
            acme.publish(name, g.view(&worker.checks(name, g.names())));
        }
        if now_secs() >= swept + SWEEP_EVERY {
            swept = now_secs();
            sweep(&plan, &groups, swept).await;
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
                    acme.publish(&name, g.view(&worker.checks(&name, g.names())));
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

fn orderable(plan: &Plan) -> BTreeMap<String, Vec<String>> {
    plan.groups
        .iter()
        .map(|(g, names)| {
            let ok: Vec<String> = names
                .iter()
                .filter(|n| {
                    super::checks_of(&plan.settings, n, plan.choices.get(g).copied()).is_ok()
                })
                .cloned()
                .collect();
            (g.clone(), ok)
        })
        .filter(|(_, names)| !names.is_empty())
        .collect()
}

async fn sync(groups: &mut BTreeMap<String, Group>, plan: &Plan) {
    let wanted = orderable(plan);
    groups.retain(|g, _| wanted.contains_key(g));
    let state = plan.settings.state_dir.clone();
    let directory = plan.settings.directory_url.clone();
    for (name, names) in &wanted {
        match groups.get_mut(name) {
            Some(g) => g.rename(names),
            None => {
                let stored = match &state {
                    Some(s) => {
                        let (s, n, d) = (s.clone(), names.clone(), directory.clone());
                        tokio::task::spawn_blocking(move || Held::find(&s, &d, &n, now_secs()))
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

async fn sweep(plan: &Plan, groups: &BTreeMap<String, Group>, now: u64) {
    let Some(state) = plan.settings.state_dir.clone() else {
        return;
    };
    let kept: Vec<PathBuf> = groups
        .values()
        .flat_map(Group::in_use)
        .map(|names| storage::cert_dir(&state, &names))
        .collect();
    let _ = tokio::task::spawn_blocking(move || storage::sweep(&state, &kept, now)).await;
}
