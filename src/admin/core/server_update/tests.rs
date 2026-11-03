use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use burrow_admin_proto::msg::AdminLevel;
use burrow_admin_proto::updates::{LatestRelease, RunMode, UpdateState, UpdateStatus};

use super::*;
use crate::admin::core::ports::{Live, LiveSession};
use crate::admin::core::tests::{FakeLive, core_running, session, world};
use crate::config::settings::ServerSettings;
use crate::metrics::Status;

struct Own {
    inner: FakeLive,
    latest: Option<String>,
    own: Mutex<ServerUpdate>,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Live for Own {
    fn status(&self) -> Status {
        self.inner.status()
    }
    fn settings(&self) -> Arc<ServerSettings> {
        self.inner.settings()
    }
    fn sessions(&self) -> Vec<LiveSession> {
        self.inner.sessions()
    }
    fn allows(&self) -> BTreeMap<String, Vec<String>> {
        BTreeMap::new()
    }
    fn free_port(&self, near: u16, count: u16, udp: bool) -> Option<u16> {
        self.inner.free_port(near, count, udp)
    }
    fn latest(&self) -> LatestRelease {
        LatestRelease {
            version: self.latest.clone(),
            ..LatestRelease::default()
        }
    }
    fn own_update(&self) -> Option<ServerUpdate> {
        Some(self.own.lock().unwrap().clone())
    }
    fn update_self(&self, by: &str) -> Result<(), String> {
        self.asked.lock().unwrap().push(by.into());
        self.own.lock().unwrap().last = Some(UpdateStatus {
            state: UpdateState::Asked,
            by: by.into(),
            ..UpdateStatus::default()
        });
        Ok(())
    }
}

fn on() -> ServerUpdate {
    ServerUpdate {
        run: RunMode::Service,
        remote: RemoteUpdates::On,
        ..ServerUpdate::default()
    }
}

fn setup(
    own: ServerUpdate,
    latest: Option<&str>,
) -> (
    Core,
    Arc<Mutex<Vec<String>>>,
    crate::admin::core::tests::World,
) {
    let w = world();
    let mut core = core_running(&w.text, &w.text);
    let asked = Arc::default();
    core.live = Box::new(Own {
        inner: FakeLive {
            text: Arc::new(Mutex::new(w.text.clone())),
            status: Mutex::new(Status {
                version: "0.2.2".into(),
                ..Status::default()
            }),
            sessions: Mutex::new(Vec::new()),
            busy: Vec::new(),
            allows: BTreeMap::new(),
        },
        latest: latest.map(String::from),
        own: Mutex::new(own),
        asked: Arc::clone(&asked),
    });
    (core, asked, w)
}

fn ask(
    core: &Core,
    s: &Session,
    dry_run: bool,
    applying: Applying,
) -> Result<UpdateServerReply, ApiError> {
    update_server(core, s, &UpdateServer { dry_run }, applying)
}

#[test]
fn a_write_admin_asks_once_after_a_preview_and_the_request_names_who_asked() {
    let (core, asked, w) = setup(on(), Some("0.2.3"));
    let s = session(&w, AdminLevel::Write);
    let preview = ask(&core, &s, true, Applying::Idle).unwrap();
    assert_eq!(
        (preview.from.as_str(), preview.to.as_str()),
        ("0.2.2", "0.2.3")
    );
    assert!(!preview.asked && preview.why_not.is_none());
    assert!(preview.note.contains("pause for a few seconds"));
    assert!(asked.lock().unwrap().is_empty(), "a preview asks nothing");
    let r = ask(&core, &s, false, Applying::Idle).unwrap();
    assert!(r.asked);
    assert_eq!(*asked.lock().unwrap(), ["ops (203.0.113.7)"]);
    let again = ask(&core, &s, false, Applying::Idle).unwrap_err();
    assert!(
        again.message.contains("already running"),
        "{}",
        again.message
    );
    assert_eq!(asked.lock().unwrap().len(), 1, "one job at a time");
    assert_eq!(
        core.status().server_update.unwrap().last.unwrap().state,
        UpdateState::Asked
    );
}

#[test]
fn read_access_cannot_update_the_server() {
    let (core, asked, w) = setup(on(), Some("0.2.3"));
    let e = ask(&core, &session(&w, AdminLevel::Read), false, Applying::Idle).unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden);
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn nothing_is_asked_while_a_config_change_is_applied() {
    let (core, asked, w) = setup(on(), Some("0.2.3"));
    let s = session(&w, AdminLevel::Write);
    let e = ask(&core, &s, false, Applying::Busy).unwrap_err();
    assert_eq!(e.message, APPLYING);
    assert_eq!(
        ask(&core, &s, true, Applying::Busy)
            .unwrap()
            .why_not
            .as_deref(),
        Some(APPLYING)
    );
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn a_server_without_the_opt_in_or_on_the_latest_release_is_not_asked_and_says_why() {
    let off = ServerUpdate {
        remote: RemoteUpdates::Off,
        why_not: Some("remote updates are off: turn them on with update.remote: true".into()),
        ..on()
    };
    let (core, asked, w) = setup(off, Some("0.2.3"));
    let s = session(&w, AdminLevel::Write);
    let e = ask(&core, &s, false, Applying::Idle).unwrap_err();
    assert!(e.message.contains("update.remote: true"), "{}", e.message);
    let (core2, asked2, _) = setup(on(), Some("0.2.2"));
    let same = ask(&core2, &s, false, Applying::Idle).unwrap_err();
    assert!(
        same.message.contains("already runs 0.2.2"),
        "{}",
        same.message
    );
    let (core3, _, _) = setup(on(), None);
    assert!(
        ask(&core3, &s, true, Applying::Idle).is_err(),
        "the latest must be known"
    );
    assert!(asked.lock().unwrap().is_empty() && asked2.lock().unwrap().is_empty());
}

#[test]
fn the_status_carries_the_servers_own_update_state() {
    let (core, _, _) = setup(on(), Some("0.2.3"));
    assert_eq!(core.status().server_update, Some(on()));
}
