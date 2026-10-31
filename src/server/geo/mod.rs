mod db;
mod keep;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use burrow_admin_proto::status::GeoipStatus;

use super::Shared;
use crate::config::settings::{GeoSource, STALE_AFTER, Service};
use crate::policy::geo::{Place, Verdict, admit};
pub(super) use db::Db;
pub(super) use keep::keep;

#[derive(Default)]
pub(super) struct Geo {
    db: RwLock<Option<Arc<Db>>>,
    watch: Mutex<Watch>,
}

#[derive(Default)]
struct Watch {
    file: Option<PathBuf>,
    checked: Option<u64>,
    result: String,
    failed: bool,
}

pub(super) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Geo {
    pub(super) fn place(&self, ip: IpAddr) -> Place {
        let held = self.db.read().expect("geoip lock").clone();
        match held {
            Some(db) => db.place(ip),
            None => Place::NoDatabase,
        }
    }

    fn held(&self) -> Option<Arc<Db>> {
        self.db.read().expect("geoip lock").clone()
    }

    fn install(&self, db: Option<Db>) {
        *self.db.write().expect("geoip lock") = db.map(Arc::new);
    }

    fn note(&self, result: impl Into<String>, failed: bool) {
        let mut w = self.watch.lock().expect("geoip lock");
        w.checked = Some(unix_now());
        w.result = result.into();
        w.failed = failed;
    }

    fn held_at(&self, file: Option<PathBuf>) {
        self.watch.lock().expect("geoip lock").file = file;
    }

    pub(super) fn status(&self, source: Option<&GeoSource>) -> Option<GeoipStatus> {
        let source = source?;
        let w = self.watch.lock().expect("geoip lock");
        let built = self.held().map(|d| d.built);
        let now = unix_now();
        Some(GeoipStatus {
            source: source.shown(),
            file: w
                .file
                .as_ref()
                .map(|f| f.display().to_string())
                .unwrap_or_default(),
            credit: source.credits_db_ip(),
            built,
            checked: w.checked,
            result: w.result.clone(),
            failed: w.failed,
            stale: built.is_some_and(|b| now.saturating_sub(b) > STALE_AFTER.as_secs()),
        })
    }
}

impl Shared {
    pub(crate) fn admits(&self, svc: &Service, ip: IpAddr) -> Verdict {
        admit(&svc.sources, svc.countries.as_ref(), ip, |ip| {
            self.geo.place(ip)
        })
    }

    pub(crate) fn admission(&self, service: &str, ip: IpAddr) -> Verdict {
        let settings = self.settings();
        let verdict = match settings.services.get(service) {
            Some(svc) => self.admits(svc, ip),
            None => Verdict::Address,
        };
        match verdict {
            Verdict::In => {}
            Verdict::Address => self.metrics.refuse(service),
            Verdict::Region => self.metrics.refuse_country(service),
        }
        verdict
    }
}

#[cfg(test)]
mod tests;
