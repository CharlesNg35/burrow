use std::path::PathBuf;

use tracing::{info, warn};

use super::Geo;
use super::db::Db;
use super::source::{Source, Stamp, Tending, stamp_of};

pub(super) struct OwnFile {
    path: PathBuf,
    stamp: Stamp,
}

impl OwnFile {
    pub(super) fn new(path: PathBuf) -> OwnFile {
        OwnFile { path, stamp: None }
    }

    fn reread(&mut self, geo: &Geo) {
        let now = stamp_of(&self.path);
        if now == self.stamp || now.is_none() {
            return;
        }
        self.stamp = now;
        match Db::open(&self.path) {
            Ok(db) => {
                info!(
                    "country database {} changed; now using it",
                    self.path.display()
                );
                geo.install(Some(db));
                geo.fine("read again after it changed");
            }
            Err(e) => {
                warn!(
                    "country database {} changed but {e}; keeping the one already read",
                    self.path.display()
                );
                geo.failed(format!("{e}; kept the one already read"));
            }
        }
    }
}

impl Source for OwnFile {
    fn file(&self) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn credits(&self) -> bool {
        false
    }

    fn start(&mut self, geo: &Geo) {
        self.stamp = stamp_of(&self.path);
        match Db::open(&self.path) {
            Ok(db) => {
                info!("country database {}", self.path.display());
                geo.install(Some(db));
                geo.fine("read from disk");
            }
            Err(e) => {
                warn!("country database {}: {e}", self.path.display());
                geo.failed(e);
            }
        }
    }

    fn tend<'a>(&'a mut self, geo: &'a Geo) -> Tending<'a> {
        self.reread(geo);
        Box::pin(std::future::ready(()))
    }
}
