use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use burrow_admin_proto::msg::AdminLevel;
use burrow_admin_proto::updates::{ClientBuild, LatestRelease, UpdateState, UpdateStatus};

use super::*;
use crate::admin::core::ports::{Live, LiveSession};
use crate::admin::core::tests::{FakeLive, core_running, session, world};
use crate::config::settings::ServerSettings;

type Asked = Arc<Mutex<Vec<(Vec<String>, String, String)>>>;

struct UpdLive {
    inner: FakeLive,
    latest: Option<String>,
    asked: Asked,
}

impl Live for UpdLive {
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
            error: self.latest.is_none().then(|| "offline".to_string()),
            checked_secs_ago: 5,
        }
    }
    fn ask_update(&self, client: &str, to: &str, by: &str) -> Result<(), String> {
        self.asked
            .lock()
            .unwrap()
            .push((vec![client.into()], to.into(), by.into()));
        Ok(())
    }
    fn update_all(&self, clients: Vec<String>, to: &str, by: &str) -> Result<(), String> {
        self.asked
            .lock()
            .unwrap()
            .push((clients, to.into(), by.into()));
        Ok(())
    }
}

fn client(name: &str, version: Option<&str>, remote: RemoteUpdates) -> ClientStatus {
    ClientStatus {
        name: name.into(),
        connected: true,
        build: version.map(|v| ClientBuild {
            version: v.into(),
            os: "linux".into(),
            arch: "amd64".into(),
            run: RunMode::Service,
            remote,
            why_not: None,
        }),
        ..ClientStatus::default()
    }
}

fn setup(
    latest: Option<&str>,
    clients: Vec<ClientStatus>,
) -> (Core, Asked, crate::admin::core::tests::World) {
    let w = world();
    let mut core = core_running(&w.text, &w.text);
    let asked: Asked = Arc::default();
    core.live = Box::new(UpdLive {
        inner: FakeLive {
            text: Arc::new(Mutex::new(w.text.clone())),
            status: Mutex::new(Status {
                clients,
                ..Status::default()
            }),
            sessions: Mutex::new(Vec::new()),
            busy: Vec::new(),
            allows: BTreeMap::new(),
        },
        latest: latest.map(String::from),
        asked: asked.clone(),
    });
    (core, asked, w)
}

#[test]
fn the_status_offers_the_latest_release_to_each_client_that_can_take_it() {
    let (core, _, _) = setup(
        Some("0.2.2"),
        vec![
            client("nas", Some("0.2.1"), RemoteUpdates::On),
            client("pi", Some("0.2.1"), RemoteUpdates::Off),
            client("new", Some("0.2.2"), RemoteUpdates::On),
            client("old", None, RemoteUpdates::Off),
        ],
    );
    let st = core.status();
    assert_eq!(
        st.latest.as_ref().unwrap().version.as_deref(),
        Some("0.2.2")
    );
    let by = |n: &str| st.clients.iter().find(|c| c.name == n).unwrap();
    assert_eq!(by("nas").available.as_deref(), Some("0.2.2"));
    assert_eq!(by("nas").blocked, None);
    assert!(
        by("pi")
            .blocked
            .as_deref()
            .unwrap()
            .contains("remote: true")
    );
    assert_eq!(by("new").available, None);
    assert_eq!(by("old").available, None);
}

#[test]
fn update_all_asks_only_the_clients_that_can_update_and_says_why_others_are_skipped() {
    let (core, asked, w) = setup(
        Some("0.2.2"),
        vec![
            client("a", Some("0.2.1"), RemoteUpdates::On),
            client("old", None, RemoteUpdates::Off),
            client("b", Some("0.2.0"), RemoteUpdates::On),
        ],
    );
    let s = session(&w, AdminLevel::Write);
    let reply = update_clients(
        &core,
        &s,
        &UpdateClients {
            all: true,
            ..UpdateClients::default()
        },
    )
    .unwrap();
    assert_eq!(reply.to, "0.2.2");
    assert_eq!(reply.asked, ["a", "b"]);
    assert_eq!(reply.skipped.len(), 1);
    assert!(reply.skipped[0].why.contains("sudo burrow update"));
    let asked = asked.lock().unwrap();
    assert_eq!(asked[0].0, ["a", "b"]);
    assert!(asked[0].2.starts_with("ops ("), "{}", asked[0].2);
}

fn asking(
    core: &Core,
    s: &Session,
    names: &[&str],
    v: Option<&str>,
    dry: bool,
) -> Result<UpdateClientsReply, ApiError> {
    update_clients(
        core,
        s,
        &UpdateClients {
            clients: names.iter().map(|n| n.to_string()).collect(),
            all: false,
            version: v.map(String::from),
            dry_run: dry,
        },
    )
}

#[test]
fn clients_are_asked_only_for_the_latest_release() {
    let mut busy = client("busy", Some("0.2.1"), RemoteUpdates::On);
    busy.update = Some(UpdateStatus {
        state: UpdateState::Downloading,
        ..UpdateStatus::default()
    });
    let (core, asked, w) = setup(
        Some("0.2.3"),
        vec![client("nas", Some("0.2.1"), RemoteUpdates::On), busy],
    );
    let s = session(&w, AdminLevel::Write);
    let older = asking(&core, &s, &["nas"], Some("0.2.2"), false).unwrap_err();
    assert!(
        older.message.contains("only to the latest release, 0.2.3"),
        "{}",
        older.message
    );
    assert!(asking(&core, &s, &["nas"], Some("../x"), false).is_err());
    let dry = asking(&core, &s, &["nas"], None, true).unwrap();
    assert_eq!(dry.asked, ["nas"]);
    assert!(asked.lock().unwrap().is_empty(), "a dry run asks nobody");
    let r = asking(&core, &s, &["nas", "busy", "ghost"], Some("v0.2.3"), false).unwrap();
    assert_eq!(r.asked, ["nas"]);
    let why: Vec<&str> = r.skipped.iter().map(|s| s.why.as_str()).collect();
    assert!(why.iter().any(|w| w.contains("already running")), "{why:?}");
    assert!(why.iter().any(|w| w.contains("no client")), "{why:?}");
    assert_eq!(asked.lock().unwrap()[0].1, "0.2.3");
    let read = session(&w, AdminLevel::Read);
    let refused = update_clients(&core, &read, &UpdateClients::default()).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Forbidden);
}

#[test]
fn nothing_is_asked_while_the_latest_release_is_unknown() {
    let (core, asked, w) = setup(None, vec![client("nas", Some("0.2.1"), RemoteUpdates::On)]);
    let s = session(&w, AdminLevel::Write);
    let unknown = asking(&core, &s, &["nas"], Some("0.2.3"), false).unwrap_err();
    assert!(
        unknown.message.contains("not known yet (offline)"),
        "{}",
        unknown.message
    );
    assert!(asked.lock().unwrap().is_empty());
}
