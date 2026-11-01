use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::time::Instant;
use tracing::{info, warn};

use super::db::{Db, MAX_PACKED, month_url, previous_month, unpack};
use super::source::{Source, Tending};
use super::{Geo, unix_now};
use crate::config::settings::{DB_IP_FILE, STALE_AFTER};
use crate::wire::fetch::{Fetched, get};

const DAY: Duration = Duration::from_secs(24 * 3600);
const JITTER_SECS: u64 = 2 * 3600;
const FIRST_RETRY: Duration = Duration::from_secs(10 * 60);
const MAX_RETRY: Duration = Duration::from_secs(12 * 3600);

pub(super) struct DbIp {
    dir: Option<PathBuf>,
    next: Instant,
    retry: Duration,
}

impl DbIp {
    pub(super) fn new(dir: Option<PathBuf>) -> DbIp {
        DbIp {
            dir,
            next: Instant::now(),
            retry: FIRST_RETRY,
        }
    }

    async fn check(&mut self, geo: &Geo, dir: &Path) {
        match refresh(geo, dir).await {
            Ok(done) => {
                geo.fine(done);
                self.next = Instant::now() + jittered_day();
                self.retry = FIRST_RETRY;
            }
            Err(e) => {
                warn!(
                    "country database: {e}; trying again in {} minutes",
                    self.retry.as_secs() / 60
                );
                geo.retrying(e);
                self.next = Instant::now() + self.retry;
                self.retry = (self.retry * 2).min(MAX_RETRY);
            }
        }
        if let Some(db) = geo.held() {
            let age = unix_now().saturating_sub(db.built);
            if age > STALE_AFTER.as_secs() {
                warn!(
                    "the country database is {} days old; burrow keeps trying to download a newer one",
                    age / 86_400
                );
            }
        }
    }
}

impl Source for DbIp {
    fn file(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(DB_IP_FILE))
    }

    fn credits(&self) -> bool {
        true
    }

    fn start(&mut self, geo: &Geo) {
        let Some(file) = self.file() else {
            geo.broken("there is no state folder to keep the database in");
            return;
        };
        if !file.exists() {
            return;
        }
        match Db::open(&file) {
            Ok(db) => {
                info!(
                    "country database {} from {}",
                    file.display(),
                    burrow_admin_proto::date::year_month(db.built)
                );
                geo.install(Some(db));
                geo.fine("read from disk");
            }
            Err(e) => {
                warn!(
                    "country database {}: {e}; downloading it again",
                    file.display()
                );
                geo.retrying(e);
            }
        }
    }

    fn tend<'a>(&'a mut self, geo: &'a Geo) -> Tending<'a> {
        Box::pin(async move {
            let Some(dir) = self.dir.clone() else {
                return;
            };
            if Instant::now() >= self.next {
                self.check(geo, &dir).await;
            }
        })
    }
}

fn jittered_day() -> Duration {
    let mut b = [0u8; 8];
    let spread = getrandom::fill(&mut b).map_or(0, |()| u64::from_le_bytes(b) % (2 * JITTER_SECS));
    DAY + Duration::from_secs(spread) - Duration::from_secs(JITTER_SECS)
}

async fn refresh(geo: &Geo, dir: &Path) -> Result<String, String> {
    let this = burrow_admin_proto::date::year_month(unix_now());
    let held = geo.held();
    let held_month = held
        .as_ref()
        .map(|d| burrow_admin_proto::date::year_month(d.built));
    if held_month.as_deref() >= Some(this.as_str()) {
        return Ok(format!("up to date ({this})"));
    }
    let mut months = vec![this.clone()];
    if held.is_none() {
        months.push(previous_month(&this));
    }
    for month in months {
        match fetch(&month_url(&month)).await? {
            Fetched::NotFound => continue,
            Fetched::Body(packed) => {
                let after = held.as_ref().map_or(0, |d| d.built);
                let dir = dir.to_path_buf();
                let db = tokio::task::spawn_blocking(move || store(&packed, &dir, after))
                    .await
                    .map_err(|e| format!("the update stopped: {e}"))??;
                info!("country database updated to DB-IP Lite {month}");
                geo.install(Some(db));
                return Ok(format!("downloaded {month}"));
            }
        }
    }
    match held_month {
        Some(m) => Ok(format!("{this} is not published yet; keeping {m}")),
        None => Err("download.db-ip.com has no file for this or last month".into()),
    }
}

async fn fetch(url: &str) -> Result<Fetched, String> {
    let env = |name: &str| std::env::var(name).ok();
    get(url, MAX_PACKED, &env).await.map_err(|e| e.to_string())
}

pub(super) fn store(packed: &[u8], dir: &Path, after: u64) -> Result<Db, String> {
    let bytes = unpack(packed)?;
    let db = Db::from_bytes(bytes.clone()).map_err(|e| format!("the download is {e}"))?;
    if db.built <= after {
        return Err("the download is not newer than the database already held".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| {
        format!(
            "cannot create {}: {}",
            dir.display(),
            crate::base::failure::file_reason(&e)
        )
    })?;
    let file = dir.join(DB_IP_FILE);
    crate::base::fs::write_atomic(&file, &bytes, 0o644).map_err(|e| {
        format!(
            "cannot write {}: {}",
            file.display(),
            crate::base::failure::file_reason(&e)
        )
    })?;
    Ok(db)
}
