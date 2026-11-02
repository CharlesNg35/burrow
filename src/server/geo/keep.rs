use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use super::Shared;
use super::source::{Source, source_for};
use crate::config::settings::GeoSource;

const TICK: Duration = Duration::from_secs(30);

pub(in crate::server) struct GeoTask(JoinHandle<()>);

impl Drop for GeoTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(in crate::server) fn start(shared: &Arc<Shared>, given: Option<&GeoSource>) -> Option<GeoTask> {
    let Some(given) = given else {
        shared.geo.reset(None);
        return None;
    };
    let mut source = source_for(given);
    shared.geo.reset(Some(source.as_ref()));
    source.start(&shared.geo);
    Some(GeoTask(tokio::spawn(keep(shared.clone(), source))))
}

async fn keep(shared: Arc<Shared>, mut source: Box<dyn Source>) {
    loop {
        source.tend(&shared.geo).await;
        tokio::time::sleep(TICK).await;
    }
}
