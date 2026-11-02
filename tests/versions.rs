mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use burrow::metrics::Status;
use burrow::wire::secret::Secret;
use burrow_admin_proto::updates::{RemoteUpdates, RunMode};

use common::{free_port, text};

const BIN: &str = env!("CARGO_BIN_EXE_burrow");
const MINE: &str = env!("CARGO_PKG_VERSION");

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Setup {
    dir: tempfile::TempDir,
    instances: PathBuf,
    listen: u16,
    secret: Secret,
}

impl Setup {
    fn new() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        Setup {
            instances: dir.path().join("instances"),
            dir,
            listen: free_port(),
            secret: Secret::generate(),
        }
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let p = self.dir.path().join(name);
        burrow::base::fs::write_private(&p, text).unwrap();
        p
    }

    fn run(&self, role: &str, config: &Path) -> Running {
        Running(
            Command::new(BIN)
                .arg(role)
                .arg("-c")
                .arg(config)
                .env("BURROW_INSTANCE_DIR", &self.instances)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn server(&self) -> PathBuf {
        self.write(
            "server.yaml",
            &format!(
                "listen: 127.0.0.1:{}\nclients:\n  home:\n    secret: {}\n  away:\n    secret: {}\n",
                self.listen,
                self.secret.encode(),
                Secret::generate().encode()
            ),
        )
    }

    fn client(&self, extra: &str) -> PathBuf {
        self.write(
            "client.yaml",
            &format!(
                "server: 127.0.0.1:{}\nname: home\nsecret: {}\n{extra}",
                self.listen,
                self.secret.encode()
            ),
        )
    }

    fn statuses(&self) -> Vec<(PathBuf, Status)> {
        let o = Command::new(BIN)
            .args(["status", "--json"])
            .env("BURROW_INSTANCE_DIR", &self.instances)
            .output()
            .unwrap();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&o.stdout).unwrap_or_default();
        list.into_iter()
            .filter_map(|v| {
                Some((
                    serde_json::from_value(v["config"].clone()).ok()?,
                    serde_json::from_value(v["status"].clone()).ok()?,
                ))
            })
            .collect()
    }

    fn wait_for(
        &self,
        what: &str,
        ok: impl Fn(&[(PathBuf, Status)]) -> bool,
    ) -> Vec<(PathBuf, Status)> {
        let start = Instant::now();
        loop {
            let got = self.statuses();
            if ok(&got) {
                return got;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "no {what}: {got:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn server_of(all: &[(PathBuf, Status)]) -> Option<&Status> {
    all.iter().map(|(_, s)| s).find(|s| s.client.is_none())
}

#[test]
fn the_server_shows_the_version_platform_and_run_mode_of_each_client() {
    let s = Setup::new();
    let _server = s.run("server", &s.server());
    let _client = s.run("client", &s.client(""));
    let all = s.wait_for("client report", |all| {
        server_of(all).is_some_and(|st| st.clients.iter().any(|c| c.build.is_some()))
    });
    let st = server_of(&all).unwrap();
    let home = st.clients.iter().find(|c| c.name == "home").unwrap();
    let build = home.build.as_ref().unwrap();
    assert_eq!(build.version, MINE);
    assert_eq!(build.os, std::env::consts::OS);
    assert!(!build.arch.is_empty());
    assert_eq!(build.run, RunMode::ByHand);
    assert_eq!(build.remote, RemoteUpdates::Off);
    let away = st.clients.iter().find(|c| c.name == "away").unwrap();
    assert!(away.build.is_none());
    let o = Command::new(BIN)
        .arg("status")
        .env("BURROW_INSTANCE_DIR", &s.instances)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let shown = text(&o);
    assert!(shown.contains("VERSION"), "{shown}");
    assert!(shown.contains("connected"), "{shown}");
}

#[test]
fn burrow_status_shows_a_running_client_and_its_remote_update_setting() {
    let s = Setup::new();
    let _server = s.run("server", &s.server());
    let client = s.client("update:\n  remote: true\n");
    let _client = s.run("client", &client);
    let all = s.wait_for("connected client", |all| {
        all.iter()
            .any(|(_, st)| st.client.as_ref().is_some_and(|c| c.connected))
    });
    let mine = all.iter().find_map(|(_, st)| st.client.clone()).unwrap();
    assert_eq!(mine.name, "home");
    assert_eq!(mine.build.version, MINE);
    assert_ne!(mine.build.remote, RemoteUpdates::Off, "{mine:?}");
    let o = Command::new(BIN)
        .args(["status", "-c"])
        .arg(&client)
        .env("BURROW_INSTANCE_DIR", &s.instances)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let shown = text(&o);
    assert!(o.status.success(), "{shown}");
    assert!(shown.contains("client \"home\" of 127.0.0.1"), "{shown}");
    assert!(shown.contains("remote updates"), "{shown}");
    assert!(shown.contains("started by hand"), "{shown}");
}
