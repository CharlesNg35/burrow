use std::collections::HashMap;
use std::sync::Arc;

use instant_acme::Account;
use tracing::{error, info, warn};

use super::dns01::solve::Dns;
pub use super::group::{Group, Held};
use super::http::{Client, Limits};
use super::manager::{Acme, Plan};
use super::order::{Ask, Failed, Issued, issue};
use super::replacing::{Next, after, cert_id, refused_text, replaces};
use super::{Problemish, schedule, storage};
use crate::tls::CertCheck;
use crate::tls::store::now_secs;

const ARI_RECHECK: u64 = 6 * 3600;
const STAGING_DELAY: u64 = 30;
const REPLAY_EVERY: u64 = 600;

pub struct Worker {
    acme: Arc<Acme>,
    plan: Option<Plan>,
    accounts: HashMap<String, (Account, Client)>,
    dns: Option<Arc<Dns>>,
    replayed: u64,
}

impl Worker {
    pub fn new(acme: Arc<Acme>) -> Worker {
        Worker {
            acme,
            plan: None,
            accounts: HashMap::new(),
            dns: None,
            replayed: 0,
        }
    }

    pub fn adopt(&mut self, plan: &Plan) {
        let same = self.plan.as_ref().is_some_and(|p| {
            p.settings.dns == plan.settings.dns && p.settings.state_dir == plan.settings.state_dir
        });
        if !same || self.dns.is_none() {
            self.dns = (!plan.settings.dns.is_empty()).then(|| {
                Arc::new(Dns::new(
                    &plan.settings.dns,
                    self.acme.dns_reach(),
                    plan.settings.state_dir.clone(),
                ))
            });
        }
        self.plan = Some(plan.clone());
    }

    pub async fn replay(&mut self) {
        let now = now_secs();
        if now < self.replayed + REPLAY_EVERY {
            return;
        }
        self.replayed = now;
        if let Some(d) = &self.dns {
            d.replay().await;
        }
    }

    pub fn checks(&self, group: &str, names: &[String]) -> Vec<CertCheck> {
        self.plan.as_ref().map_or_else(Vec::new, |p| {
            super::checks_for_all(&p.settings, names, p.choices.get(group).copied())
        })
    }

    pub fn forget_accounts(&mut self) {
        self.accounts.clear();
    }

    async fn account(&mut self, directory: &str) -> Result<(Account, Client), String> {
        if let Some((a, c)) = self.accounts.get(directory) {
            return Ok((a.clone(), c.clone()));
        }
        let plan = self.plan.as_ref().ok_or("no ACME settings")?;
        let state = plan
            .settings
            .state_dir
            .clone()
            .ok_or("burrow has no state folder for certificates")?;
        let client = Client::new(plan.settings.ca.as_deref(), Limits::default())?;
        let opened = super::account::open(&client, &state, directory, &plan.settings.email).await?;
        if opened.created {
            info!(
                "ACME account {} created at {directory}",
                opened.account.id()
            );
        }
        if directory == plan.settings.directory_url {
            self.acme.set_account(Some(opened.account.id().to_string()));
        }
        self.accounts.insert(
            directory.to_string(),
            (opened.account.clone(), client.clone()),
        );
        Ok((opened.account, client))
    }

    pub async fn step(&mut self, name: &str, g: &mut Group) {
        let now = now_secs();
        if g.staging_at.is_some_and(|t| t <= now) {
            self.staging(name, g).await;
            return;
        }
        if !g.needs_order(now) {
            self.ari(g).await;
            if !g.needs_order(now_secs()) {
                return;
            }
        }
        if self.reuse(g).await {
            return;
        }
        g.running = true;
        self.acme
            .publish(name, g.view(&self.checks(name, &g.names)));
        let result = self.order(name, g).await;
        g.running = false;
        match result {
            Ok(held) => {
                g.code = None;
                self.done(name, g, held).await;
            }
            Err((text, wait)) => {
                g.code = text.starts_with(NOT_OFFERED).then_some(PROFILE_NOT_OFFERED);
                self.failed(name, g, text, wait);
            }
        }
    }

    async fn reuse(&mut self, g: &mut Group) -> bool {
        let Some(plan) = &self.plan else {
            return false;
        };
        let Some(state) = plan.settings.state_dir.clone() else {
            return false;
        };
        let (directory, names, held) = (
            plan.settings.directory_url.clone(),
            g.names.clone(),
            g.held.clone(),
        );
        let found = tokio::task::spawn_blocking(move || {
            Held::reusable(&state, &directory, &names, now_secs(), held.as_ref())
        })
        .await
        .ok()
        .flatten();
        let Some(h) = found else {
            return false;
        };
        info!(
            "certificate for {}: a stored certificate for {} covers it, so nothing is ordered",
            g.names.join(", "),
            h.names.join(", ")
        );
        self.acme.serve(&g.names, &h.key);
        g.held = Some(h);
        g.planned = None;
        g.failures = 0;
        g.last_error = None;
        g.replaces_refused = 0;
        g.next = now_secs();
        true
    }

    async fn ari(&mut self, g: &mut Group) {
        let Some(h) = g.held.clone() else {
            return;
        };
        let now = now_secs();
        let fixed = schedule::renew_at(h.not_before, h.not_after);
        let Some(id) = h.ari_id.as_deref().and_then(cert_id) else {
            g.planned = Some(fixed);
            g.next = fixed;
            return;
        };
        let directory = self.plan.as_ref().map(|p| p.settings.directory_url.clone());
        let account = match directory {
            Some(d) => self.account(&d).await,
            None => return,
        };
        let asked = match &account {
            Ok((a, _)) => Some(a.renewal_info(&id).await),
            Err(_) => None,
        };
        match asked {
            Some(Ok((info, retry))) => {
                let start = info.suggested_window.start.unix_timestamp().max(0) as u64;
                let end = info.suggested_window.end.unix_timestamp().max(0) as u64;
                if end <= start {
                    g.planned = Some(g.planned.unwrap_or(fixed));
                    g.next = g.planned.unwrap_or(fixed).min(now + ARI_RECHECK);
                    return;
                }
                let pick = schedule::in_window(start, end, super::fraction());
                g.explanation = info.explanation_url;
                g.planned = Some(pick);
                g.next = pick.min(now + schedule::ari_next_check(Some(retry)).as_secs());
            }
            Some(Err(e)) if Problemish::of(&e) == Problemish::Unsupported => {
                g.planned = Some(fixed);
                g.next = fixed;
            }
            _ => {
                let planned = g.planned.unwrap_or(fixed);
                g.planned = Some(planned);
                g.next = planned.min(now + schedule::ARI_LONG_TERM_ERROR.as_secs());
            }
        }
    }

    async fn order(
        &mut self,
        name: &str,
        g: &mut Group,
    ) -> Result<(Held, CertCheck, Issued), (String, Option<std::time::Duration>)> {
        let plan = self
            .plan
            .clone()
            .ok_or(("no ACME settings".to_string(), None))?;
        let directory = plan.settings.directory_url.clone();
        let (account, client) = self.account(&directory).await.map_err(|e| (e, None))?;
        let profile = profile_for(&account, &g.names, plan.settings.profile.as_deref())
            .map_err(|e| (e, None))?;
        let mut checks =
            super::checks_for_all(&plan.settings, &g.names, plan.choices.get(name).copied());
        let learned = self.learned(&g.names, &checks).await;
        if let Some(c) = learned.or(g.check)
            && let Some(i) = checks.iter().position(|x| *x == c)
        {
            let c = checks.remove(i);
            checks.insert(0, c);
        }
        if checks.is_empty() {
            return Err((
                "no port for Let's Encrypt's checks (web.https on 443 or web.http on 80)".into(),
                None,
            ));
        }
        let replaces = replaces(g, now_secs());
        let dns = super::dns_names(&plan.settings, &g.names, plan.choices.get(name).copied());
        let mut last: Option<Failed> = None;
        for check in checks {
            let mut with = replaces.clone();
            loop {
                if with.is_none() {
                    self.guard(&g.names).await?;
                }
                let ask = Ask {
                    names: &g.names,
                    check,
                    dns: &dns,
                    profile: profile.as_deref(),
                    replaces: with.clone(),
                };
                match issue(&account, &self.acme.challenges, self.dns.as_ref(), ask).await {
                    Ok(issued) => {
                        self.note_issued(&g.names, with.is_some()).await;
                        self.learn(&g.names, check).await;
                        return held_of(&issued, &g.names, check).map_err(|e| (e, None));
                    }
                    Err(f) => match after(with.is_some(), &f) {
                        Next::PlainNow => with = None,
                        Next::OtherCheck => {
                            last = Some(f);
                            break;
                        }
                        Next::Refused => {
                            g.replaces_refused = g.replaces_refused.saturating_add(1);
                            return Err((refused_text(f.text, g.replaces_refused), None));
                        }
                        Next::Stop => {
                            let wait = (f.kind == Problemish::RateLimited)
                                .then(|| client.take_wait())
                                .flatten();
                            return Err((f.text, wait));
                        }
                    },
                }
            }
        }
        Err((
            last.map_or_else(|| "the order failed".into(), |f| f.text),
            None,
        ))
    }

    fn guard_path(&self) -> Option<std::path::PathBuf> {
        let p = self.plan.as_ref()?;
        Some(super::guard::path(
            p.settings.state_dir.as_ref()?,
            &p.settings.directory_url,
        ))
    }

    async fn guard(&self, names: &[String]) -> Result<(), (String, Option<std::time::Duration>)> {
        let Some(path) = self.guard_path() else {
            return Ok(());
        };
        let list = tokio::task::spawn_blocking(move || super::guard::load(&path))
            .await
            .unwrap_or_default();
        let now = now_secs();
        match super::guard::next_allowed(&list, names, now) {
            None => Ok(()),
            Some(next) => Err((
                super::guard::refusal(names, next),
                Some(std::time::Duration::from_secs(next.saturating_sub(now))),
            )),
        }
    }

    fn learned_path(&self) -> Option<std::path::PathBuf> {
        let p = self.plan.as_ref()?;
        Some(super::learned::path(
            p.settings.state_dir.as_ref()?,
            &p.settings.directory_url,
        ))
    }

    async fn learned(&self, names: &[String], allowed: &[CertCheck]) -> Option<CertCheck> {
        let path = self.learned_path()?;
        let learned = tokio::task::spawn_blocking(move || super::learned::load(&path))
            .await
            .unwrap_or_default();
        super::learned::first(&learned, names, allowed)
    }

    async fn learn(&self, names: &[String], check: CertCheck) {
        let Some(path) = self.learned_path() else {
            return;
        };
        let names = names.to_vec();
        let _ = tokio::task::spawn_blocking(move || super::learned::remember(&path, &names, check))
            .await;
    }

    async fn note_issued(&self, names: &[String], ari: bool) {
        let Some(path) = self.guard_path() else {
            return;
        };
        let names = names.to_vec();
        let saved = tokio::task::spawn_blocking(move || {
            super::guard::record(&path, &names, ari, now_secs())
        })
        .await;
        if !matches!(saved, Ok(Ok(()))) {
            warn!("the list of issued certificates could not be saved in the state folder");
        }
    }

    async fn done(&mut self, name: &str, g: &mut Group, held: (Held, CertCheck, Issued)) {
        let (held, check, issued) = held;
        if let Some(plan) = &self.plan
            && let Some(state) = plan.settings.state_dir.clone()
        {
            let stored = storage::Stored {
                chain_pem: issued.chain_pem,
                key_pem: issued.key_pem,
                meta: storage::Meta {
                    directory: plan.settings.directory_url.clone(),
                    names: g.names.clone(),
                    issued: now_secs(),
                },
            };
            let dir = storage::cert_dir(&state, &g.names);
            let saved = tokio::task::spawn_blocking(move || storage::save(&dir, &stored)).await;
            if !matches!(saved, Ok(Ok(()))) {
                error!(
                    "certificate for {name}: issued, but it could not be saved in {}; it is served until burrow restarts",
                    state.display()
                );
            }
        }
        self.acme.serve(&g.names, &held.key);
        info!("{}", issued_text(&g.names, held.not_after));
        g.held = Some(held);
        g.failures = 0;
        g.last_error = None;
        g.check = Some(check);
        g.planned = None;
        g.next = now_secs() + ARI_RECHECK;
        g.staging_at = None;
        g.prod_failed_at = None;
        g.fallback_note = None;
        g.replaces_refused = 0;
    }

    fn failed(
        &mut self,
        name: &str,
        g: &mut Group,
        text: String,
        wait: Option<std::time::Duration>,
    ) {
        let now = now_secs();
        g.failures = g.failures.saturating_add(1);
        g.total += 1;
        let left = g.held.as_ref().map(|h| h.not_after.saturating_sub(now));
        let short = g.held.as_ref().map_or_else(
            || {
                g.names
                    .iter()
                    .any(|n| n.parse::<std::net::IpAddr>().is_ok())
            },
            Held::short_lived,
        );
        let delay = self.acme.backoff(schedule::jittered(
            schedule::after_failure(g.failures, short, left, wait),
            super::fraction(),
        ));
        g.next = now + delay.as_secs();
        let fallback = self
            .plan
            .as_ref()
            .is_some_and(|p| p.settings.fallback_directory.is_some());
        if fallback && delay.as_secs() > STAGING_DELAY {
            g.prod_failed_at = Some(now);
            g.staging_at = Some(now + STAGING_DELAY);
        }
        let next = delay.as_secs() / 60;
        match left {
            Some(l) if schedule::urgent(l, short) => error!(
                "certificate for {name}: {text}; it expires in {} hours; next try in {next} min",
                l / 3600
            ),
            _ => warn!("certificate for {name}: {text}; next try in {next} min"),
        }
        g.last_error = Some(text);
    }

    async fn staging(&mut self, name: &str, g: &mut Group) {
        g.staging_at = None;
        let Some(check) = g
            .check
            .or_else(|| self.checks(name, &g.names).first().copied())
        else {
            return;
        };
        let Some(directory) = self
            .plan
            .as_ref()
            .and_then(|p| p.settings.fallback_directory.clone())
        else {
            return;
        };
        let account = match self.account(&directory).await {
            Ok((a, _)) => a,
            Err(e) => {
                g.fallback_note = Some(format!(
                    "the test service {directory} could not be reached: {e}"
                ));
                return;
            }
        };
        let dns = self.plan.as_ref().map_or_else(Vec::new, |p| {
            super::dns_names(&p.settings, &g.names, p.choices.get(name).copied())
        });
        let ask = Ask {
            names: &g.names,
            check,
            dns: &dns,
            profile: None,
            replaces: None,
        };
        match issue(&account, &self.acme.challenges, self.dns.as_ref(), ask).await {
            Ok(_) => {
                let earliest = g.prod_failed_at.unwrap_or(0) + schedule::BACKOFF_SECS[2];
                if g.next > earliest {
                    g.next = earliest.max(now_secs());
                }
                g.fallback_note = Some(format!(
                    "the test service {directory} accepted the same checks, so this server can be reached"
                ));
                info!(
                    "certificate for {name}: the test service {directory} accepted the same checks"
                );
            }
            Err(f) => {
                g.fallback_note = Some(format!(
                    "the test service {directory} failed too: {}",
                    f.text
                ));
                info!(
                    "certificate for {name}: the test service {directory} failed too: {}",
                    f.text
                );
            }
        }
    }
}

fn held_of(
    issued: &Issued,
    names: &[String],
    check: CertCheck,
) -> Result<(Held, CertCheck, Issued), String> {
    let held = Held::parse(&issued.chain_pem, &issued.key_pem, names)?;
    Ok((held, check, issued.clone()))
}

pub const PROFILE_NOT_OFFERED: &str = "profile-not-offered";
const NOT_OFFERED: &str = "the certificate service does not offer the profile";

pub fn profile_for(
    account: &Account,
    names: &[String],
    configured: Option<&str>,
) -> Result<Option<String>, String> {
    let offered: Vec<String> = account.profiles().map(|p| p.name.to_string()).collect();
    choose_profile(&offered, names, configured)
}

pub fn choose_profile(
    offered: &[String],
    names: &[String],
    configured: Option<&str>,
) -> Result<Option<String>, String> {
    let has = |p: &str| offered.iter().any(|o| o == p);
    if names.iter().any(|n| n.parse::<std::net::IpAddr>().is_ok()) {
        return Ok(has("shortlived").then(|| "shortlived".to_string()));
    }
    match configured {
        None => Ok(None),
        Some(p) if has(p) => Ok(Some(p.to_string())),
        Some(p) => Err(format!(
            "{NOT_OFFERED} \"{p}\"; it offers: {}",
            if offered.is_empty() {
                "none".to_string()
            } else {
                offered.join(", ")
            }
        )),
    }
}

fn issued_text(names: &[String], not_after: u64) -> String {
    let until = std::time::UNIX_EPOCH + std::time::Duration::from_secs(not_after);
    format!(
        "certificate for {} issued, valid until {}",
        names.join(", "),
        crate::log::rfc3339_time(until)
    )
}

#[cfg(test)]
mod tests;
