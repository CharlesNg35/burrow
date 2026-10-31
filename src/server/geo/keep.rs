use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::time::Instant;
use tracing::{info, warn};

use super::db::{Db, MAX_PACKED, month_url, previous_month, unpack};
use super::{Geo, Shared, unix_now};
use crate::config::settings::{DB_IP_FILE, GeoSource, STALE_AFTER};
use crate::wire::fetch::{Fetched, get};

const TICK: Duration = Duration::from_secs(30);
const DAY: Duration = Duration::from_secs(24 * 3600);
const JITTER_SECS: u64 = 2 * 3600;
const FIRST_RETRY: Duration = Duration::from_secs(10 * 60);
const MAX_RETRY: Duration = Duration::from_secs(12 * 3600);

type Stamp = Option<(SystemTime, u64)>;

struct Plan {
    held: Option<GeoSource>,
    stamp: Stamp,
    next: Instant,
    retry: Duration,
}

pub async fn keep(shared: Arc<Shared>) {
    let mut shutdown = shared.shutdown.clone();
    let mut plan = Plan {
        held: None,
        stamp: None,
        next: Instant::now(),
        retry: FIRST_RETRY,
    };
    loop {
        let source = shared.settings().geoip.clone();
        if source != plan.held {
            plan = Plan {
                held: source,
                stamp: None,
                next: Instant::now(),
                retry: FIRST_RETRY,
            };
            start(&shared, &mut plan);
        }
        match plan.held.clone() {
            Some(GeoSource::File(path)) => reread(&shared.geo, &path, &mut plan.stamp),
            Some(GeoSource::DbIp { dir: Some(dir) }) if Instant::now() >= plan.next => {
                check(&shared, &dir, &mut plan).await;
            }
            _ => {}
        }
        tokio::select! {
            _ = tokio::time::sleep(TICK) => {}
            _ = shutdown.changed() => return,
        }
    }
}

fn file_of(source: &GeoSource) -> Option<PathBuf> {
    match source {
        GeoSource::DbIp { dir } => dir.as_ref().map(|d| d.join(DB_IP_FILE)),
        GeoSource::File(p) => Some(p.clone()),
    }
}

fn stamp_of(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn start(shared: &Shared, plan: &mut Plan) {
    let geo = &shared.geo;
    let Some(source) = plan.held.as_ref() else {
        geo.install(None);
        geo.held_at(None);
        return;
    };
    let file = file_of(source);
    geo.held_at(file.clone());
    let Some(file) = file else {
        geo.install(None);
        geo.note("there is no state folder to keep the database in", true);
        return;
    };
    plan.stamp = stamp_of(&file);
    match Db::open(&file) {
        Ok(db) => {
            info!(
                "country database {} from {}",
                file.display(),
                burrow_admin_proto::date::year_month(db.built)
            );
            geo.install(Some(db));
            geo.note("read from disk", false);
        }
        Err(_) if plan.stamp.is_none() && source.credits_db_ip() => {
            geo.install(None);
        }
        Err(e) => {
            warn!("country database {}: {e}", file.display());
            geo.install(None);
            geo.note(e, true);
        }
    }
}

pub(super) fn reread(geo: &Geo, path: &Path, stamp: &mut Stamp) {
    let now = stamp_of(path);
    if now == *stamp || now.is_none() {
        return;
    }
    *stamp = now;
    match Db::open(path) {
        Ok(db) => {
            info!("country database {} changed; now using it", path.display());
            geo.install(Some(db));
            geo.note("read again after it changed", false);
        }
        Err(e) => {
            warn!(
                "country database {} changed but {e}; keeping the one already read",
                path.display()
            );
            geo.note(format!("{e}; kept the one already read"), true);
        }
    }
}

fn jittered_day() -> Duration {
    let mut b = [0u8; 8];
    let spread = getrandom::fill(&mut b).map_or(0, |()| u64::from_le_bytes(b) % (2 * JITTER_SECS));
    DAY + Duration::from_secs(spread) - Duration::from_secs(JITTER_SECS)
}

async fn check(shared: &Shared, dir: &Path, plan: &mut Plan) {
    match refresh(shared, dir).await {
        Ok(done) => {
            shared.geo.note(done, false);
            plan.next = Instant::now() + jittered_day();
            plan.retry = FIRST_RETRY;
        }
        Err(e) => {
            warn!(
                "country database: {e}; trying again in {} minutes",
                plan.retry.as_secs() / 60
            );
            shared.geo.note(e, true);
            plan.next = Instant::now() + plan.retry;
            plan.retry = (plan.retry * 2).min(MAX_RETRY);
        }
    }
    if let Some(db) = shared.geo.held() {
        let age = unix_now().saturating_sub(db.built);
        if age > STALE_AFTER.as_secs() {
            warn!(
                "the country database is {} days old; burrow keeps trying to download a newer one",
                age / 86_400
            );
        }
    }
}

async fn refresh(shared: &Shared, dir: &Path) -> Result<String, String> {
    let this = burrow_admin_proto::date::year_month(unix_now());
    let held = shared.geo.held();
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
        let url = month_url(&month);
        match fetch(&url).await? {
            Fetched::NotFound => continue,
            Fetched::Body(packed) => {
                let after = held.as_ref().map_or(0, |d| d.built);
                let dir = dir.to_path_buf();
                let db = tokio::task::spawn_blocking(move || store(&packed, &dir, after))
                    .await
                    .map_err(|e| format!("the update stopped: {e}"))??;
                info!("country database updated to DB-IP Lite {month}");
                shared.geo.install(Some(db));
                return Ok(format!("downloaded {month}"));
            }
        }
    }
    match held_month {
        Some(m) => Ok(format!("{this} is not published yet; keeping {m}")),
        None => Err("download.db-ip.com has no file for this or last month".into()),
    }
}

pub(super) fn store(packed: &[u8], dir: &Path, after: u64) -> Result<Db, String> {
    let bytes = unpack(packed)?;
    let db = Db::from_bytes(bytes.clone()).map_err(|e| format!("the download is {e}"))?;
    if db.built <= after {
        return Err("the download is not newer than the database already held".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let file = dir.join(DB_IP_FILE);
    crate::base::fs::write_atomic(&file, &bytes, 0o644)
        .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    Ok(db)
}

async fn fetch(url: &str) -> Result<Fetched, String> {
    let env = |name: &str| std::env::var(name).ok();
    get(url, MAX_PACKED, &env).await.map_err(|e| e.to_string())
}
