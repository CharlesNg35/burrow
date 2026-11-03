use std::path::Path;
use std::sync::{Arc, OnceLock};

use burrow_admin_proto::updates::ServerUpdate;

use crate::config::settings::ServerSettings;

pub trait OwnUpdate: Send + Sync {
    fn state(&self, settings: &ServerSettings) -> ServerUpdate;
    fn ask(&self, settings: &ServerSettings, by: &str) -> Result<(), String>;
}

type Make = fn(&Path) -> Arc<dyn OwnUpdate>;

static MAKE: OnceLock<Make> = OnceLock::new();

pub fn use_own_update(make: Make) {
    let _ = MAKE.set(make);
}

pub fn for_config(path: &Path) -> Option<Arc<dyn OwnUpdate>> {
    MAKE.get().map(|make| make(path))
}
