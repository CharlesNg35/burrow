use std::sync::Arc;
use std::time::Duration;

use super::Shared;
use super::source::{Source, source_for};
use crate::config::settings::GeoSource;

const TICK: Duration = Duration::from_secs(30);

pub async fn keep(shared: Arc<Shared>) {
    let mut shutdown = shared.shutdown.clone();
    let mut held: Option<GeoSource> = None;
    let mut source: Option<Box<dyn Source>> = None;
    loop {
        let given = shared.settings().geoip.clone();
        if given != held {
            source = given.as_ref().map(source_for);
            shared.geo.reset(source.as_deref());
            if let Some(s) = source.as_mut() {
                s.start(&shared.geo);
            }
            held = given;
        }
        if let Some(s) = source.as_mut() {
            s.tend(&shared.geo).await;
        }
        tokio::select! {
            _ = tokio::time::sleep(TICK) => {}
            _ = shutdown.changed() => return,
        }
    }
}
