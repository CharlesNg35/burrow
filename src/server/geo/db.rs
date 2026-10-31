use std::io::Read;
use std::net::IpAddr;
use std::path::Path;

use crate::policy::geo::Place;

pub const MAX_PACKED: usize = 64 * 1024 * 1024;
pub const MAX_UNPACKED: u64 = 256 * 1024 * 1024;
const PROBES: [IpAddr; 3] = [
    IpAddr::V4(std::net::Ipv4Addr::new(8, 8, 8, 8)),
    IpAddr::V4(std::net::Ipv4Addr::new(1, 1, 1, 1)),
    IpAddr::V4(std::net::Ipv4Addr::new(9, 9, 9, 9)),
];

pub struct Db {
    reader: maxminddb::Reader<Vec<u8>>,
    pub built: u64,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").field("built", &self.built).finish()
    }
}

impl Db {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Db, String> {
        let reader = maxminddb::Reader::from_source(bytes)
            .map_err(|e| format!("not a MaxMind-format database: {e}"))?;
        let kind = reader.metadata().database_type.to_ascii_lowercase();
        if !kind.contains("country") && !kind.contains("city") {
            return Err(format!(
                "a {} database, not a country database",
                reader.metadata().database_type
            ));
        }
        let db = Db {
            built: reader.metadata().build_epoch,
            reader,
        };
        if !PROBES
            .iter()
            .any(|ip| matches!(db.place(*ip), Place::Known(_)))
        {
            return Err("it knows no country for well-known public addresses".into());
        }
        Ok(db)
    }

    pub fn open(path: &Path) -> Result<Db, String> {
        let f = std::fs::File::open(path).map_err(|e| format!("cannot open it: {e}"))?;
        let mut bytes = Vec::new();
        f.take(MAX_UNPACKED + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("cannot read it: {e}"))?;
        if bytes.len() as u64 > MAX_UNPACKED {
            return Err(format!("it is larger than {} MiB", MAX_UNPACKED >> 20));
        }
        Db::from_bytes(bytes)
    }

    pub fn place(&self, ip: IpAddr) -> Place {
        let found = self
            .reader
            .lookup(ip.to_canonical())
            .ok()
            .and_then(|r| {
                r.decode_path::<String>(&maxminddb::path!["country", "iso_code"])
                    .ok()
            })
            .flatten();
        match found {
            Some(code) => Place::Known(code.to_ascii_uppercase()),
            None => Place::Unknown,
        }
    }
}

pub fn unpack(packed: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(packed)
        .take(MAX_UNPACKED + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("cannot unpack it: {e}"))?;
    if out.len() as u64 > MAX_UNPACKED {
        return Err(format!(
            "it unpacks to more than {} MiB",
            MAX_UNPACKED >> 20
        ));
    }
    Ok(out)
}

pub fn month_url(month: &str) -> String {
    format!("https://download.db-ip.com/free/dbip-country-lite-{month}.mmdb.gz")
}

pub fn previous_month(month: &str) -> String {
    let (y, m) = month.split_once('-').unwrap_or(("1970", "01"));
    let (y, m): (i64, u32) = (y.parse().unwrap_or(1970), m.parse().unwrap_or(1));
    if m <= 1 {
        format!("{:04}-12", y - 1)
    } else {
        format!("{y:04}-{:02}", m - 1)
    }
}
