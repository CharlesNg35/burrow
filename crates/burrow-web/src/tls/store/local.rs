use std::collections::BTreeMap;
use std::time::Duration;

use super::super::internal::{CHECK_EVERY_SECS, Writer, default_writer};
use super::{CertOrigin, CertPhase, CertStatus, Slot, Store, now_secs};
use crate::CertSettings;

impl Store {
    pub(super) fn private_writer(&self) -> Writer {
        default_writer()
    }

    pub(super) async fn run_local(&self, settings: &CertSettings) {
        let groups: BTreeMap<String, Vec<String>> = {
            let inner = self.inner.read().expect("certificate lock");
            let mut g: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for n in &inner.order {
                if let Some(s) = inner.slots.get(n).filter(|s| s.internal) {
                    g.entry(s.group.clone()).or_default().push(n.clone());
                }
            }
            g
        };
        self.local.plan(settings.state_dir.clone(), groups);
        let mut task = self.local_task.lock().expect("task lock");
        if !self.local.active() {
            if let Some(t) = task.take() {
                t.abort();
            }
            return;
        }
        drop(task);
        let (local, writer) = (self.local.clone(), self.private_writer());
        let _ = tokio::task::spawn_blocking(move || local.step(now_secs(), &*writer)).await;
        let mut task = self.local_task.lock().expect("task lock");
        if task.is_none() {
            let (local, writer) = (self.local.clone(), self.private_writer());
            *task = Some(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(CHECK_EVERY_SECS)).await;
                    let (l, w) = (local.clone(), writer.clone());
                    let _ = tokio::task::spawn_blocking(move || l.step(now_secs(), &*w)).await;
                }
            }));
        }
    }

    pub(super) fn local_status(&self, name: &str, s: &Slot) -> CertStatus {
        let leaf = self.local.leaf(&s.group);
        let error = self.local.error();
        let phase = match (&leaf, &error) {
            (Some(_), _) => CertPhase::Ok,
            (None, Some(e)) => CertPhase::Failing { reason: e.clone() },
            (None, None) => CertPhase::Pending,
        };
        CertStatus {
            name: name.to_string(),
            origin: CertOrigin::Internal,
            check: None,
            phase,
            expires: leaf.as_ref().map(|l| l.not_after),
            next_renewal: leaf
                .as_ref()
                .map(|l| super::super::internal::leaf_renew_at(l.not_before, l.not_after)),
            last_error: error,
            code: None,
            explanation: None,
            failures: 0,
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        if let Some(t) = self.local_task.lock().ok().and_then(|mut t| t.take()) {
            t.abort();
        }
    }
}
