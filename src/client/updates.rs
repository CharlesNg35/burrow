use burrow_admin_proto::updates::LocalClient;
use tokio::sync::{mpsc, watch};

use super::Presence;
use crate::metrics::Status;

use crate::config::settings::ClientSettings;
use crate::wire::protocol::{Remote, Report, RunMode, Step};

pub type Progress = mpsc::UnboundedSender<(u32, Step, String)>;

pub trait Updates: Send + Sync {
    fn report(&self, settings: &ClientSettings) -> Report;
    fn connected(&self) {}
    fn attach(&self, _report_again: std::sync::Arc<watch::Sender<u64>>) {}
    fn ask(
        &self,
        settings: &ClientSettings,
        id: u32,
        version: &str,
        progress: Progress,
    ) -> Result<(), String> {
        let _ = (settings, id, version, progress);
        Err("this client does not take updates from the server".into())
    }
    fn stop_wanted(&self) -> Option<watch::Receiver<bool>> {
        None
    }
    fn before_start(&self) -> Option<Result<(), String>> {
        None
    }
    fn after_stop(&self) -> Option<Result<u8, String>> {
        None
    }
}

pub struct Fixed;

impl Updates for Fixed {
    fn report(&self, settings: &ClientSettings) -> Report {
        Report {
            version: crate::base::build::version().to_string(),
            os: crate::base::build::os().to_string(),
            arch: crate::base::build::arch().to_string(),
            run: RunMode::ByHand,
            remote: if settings.remote_updates {
                Remote::Blocked("this program cannot update itself".into())
            } else {
                Remote::Off
            },
            last: None,
        }
    }
}

pub fn local_status(
    presence: &Presence,
    updates: &dyn Updates,
    started: std::time::Instant,
) -> Status {
    let settings = presence.settings();
    let report = updates.report(&settings);
    let connected = presence.connected_secs();
    Status {
        version: crate::base::build::version().to_string(),
        uptime_secs: started.elapsed().as_secs(),
        client: Some(LocalClient {
            name: settings.name.clone(),
            server: settings.server.clone(),
            connected: connected.is_some(),
            connected_secs: connected,
            build: report.build(),
            last_update: report
                .last
                .as_ref()
                .map(crate::wire::protocol::LastUpdate::status),
        }),
        ..Status::default()
    }
}
