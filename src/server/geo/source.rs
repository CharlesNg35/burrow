use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::SystemTime;

use super::Geo;
use super::db_ip::DbIp;
use super::own_file::OwnFile;
use crate::config::settings::GeoSource;

pub(super) type Tending<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub(super) trait Source: Send {
    fn file(&self) -> Option<PathBuf>;

    fn credits(&self) -> bool;

    fn start(&mut self, geo: &Geo);

    fn tend<'a>(&'a mut self, geo: &'a Geo) -> Tending<'a>;
}

pub(super) fn source_for(given: &GeoSource) -> Box<dyn Source> {
    match given {
        GeoSource::DbIp { dir } => Box::new(DbIp::new(dir.clone())),
        GeoSource::File(path) => Box::new(OwnFile::new(path.clone())),
    }
}

pub(super) type Stamp = Option<(SystemTime, u64)>;

pub(super) fn stamp_of(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}
