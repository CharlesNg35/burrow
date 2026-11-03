use std::path::PathBuf;
use std::time::Duration;

pub const DB_IP: &str = "db-ip";
pub const DB_IP_FOLDER: &str = "geoip";
pub const DB_IP_FILE: &str = "dbip-country-lite.mmdb";
pub const DB_IP_SITE: &str = "https://db-ip.com";
pub const DB_IP_CREDIT: &str = "IP geolocation by DB-IP";
pub const STALE_AFTER: Duration = Duration::from_secs(45 * 24 * 3600);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GeoSource {
    DbIp { dir: Option<PathBuf> },
    File(PathBuf),
}

impl GeoSource {
    pub fn shown(&self) -> String {
        match self {
            GeoSource::DbIp { .. } => DB_IP.to_string(),
            GeoSource::File(p) => p.display().to_string(),
        }
    }
}
