use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::common::releases::Releases;
use super::{Admin, World, connect, world};

const OLD: &str = "9.0.0-test.1";
const NEW: &str = "9.0.0-test.2";

fn build(marks: &[(&str, &str)]) -> Vec<u8> {
    let mut b = std::fs::read(env!("CARGO_BIN_EXE_burrow")).unwrap();
    b.push(b'\n');
    for (k, v) in marks {
        b.extend_from_slice(format!("BURROW-TEST-{k}={v}\n").as_bytes());
    }
    b
}

struct Hand {
    dir: tempfile::TempDir,
    binary: PathBuf,
    config: PathBuf,
    releases: Releases,
    keep: Arc<AtomicBool>,
    supervisor: Option<std::thread::JoinHandle<()>>,
}

impl Hand {
    fn new(w: &World) -> Hand {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join(if cfg!(windows) {
            "burrow.exe"
        } else {
            "burrow"
        });
        std::fs::write(&binary, build(&[("VERSION", OLD)])).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let server = if super::common::ws_mode() {
            format!("ws://{}/burrow", w.addr)
        } else {
            w.addr.to_string()
        };
        let config = dir.path().join("client.yaml");
        burrow::base::fs::write_private(
            &config,
            &format!(
                "server: {server}\nname: home\nsecret: {}\nallow: [127.0.0.1/32]\nupdate:\n  remote: true\n",
                w.home.encode()
            ),
        )
        .unwrap();
        Hand {
            dir,
            binary,
            config,
            releases: Releases::start(),
            keep: Arc::new(AtomicBool::new(true)),
            supervisor: None,
        }
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.binary);
        c.args(["client", "-c"])
            .arg(&self.config)
            .env("BURROW_UPDATE_URL", &self.releases.base)
            .env("BURROW_INSTANCE_DIR", self.dir.path().join("instances"))
            .env("BURROW_ADMIN_TEST_UPDATE_WINDOW", "4")
            .env_remove("HTTPS_PROXY")
            .env_remove("https_proxy")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        c
    }

    fn supervise(&mut self) {
        let mut command = self.command();
        let keep = self.keep.clone();
        self.supervisor = Some(std::thread::spawn(move || {
            while keep.load(Ordering::Acquire) {
                let Ok(mut child): std::io::Result<Child> = command.spawn() else {
                    return;
                };
                while keep.load(Ordering::Acquire) {
                    if child.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = child.kill();
                let _ = child.wait();
                std::thread::sleep(Duration::from_millis(300));
            }
        }));
    }

    fn record(&self) -> Value {
        let path = self.binary.with_extension("update.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_default()).unwrap_or_default()
    }
}

impl Drop for Hand {
    fn drop(&mut self) {
        self.keep.store(false, Ordering::Release);
        if let Some(s) = self.supervisor.take() {
            let _ = s.join();
        }
    }
}

async fn until(a: &mut Admin, what: &str, ok: impl Fn(&Value) -> bool) -> Value {
    let start = Instant::now();
    loop {
        let status = a.event("status").await;
        let home = status["clients"]
            .as_array()
            .and_then(|c| c.iter().find(|c| c["name"] == "home").cloned())
            .unwrap_or_default();
        if ok(&home) {
            return home;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "no {what}: {home}"
        );
    }
}

fn latest() -> burrow_admin_proto::updates::LatestRelease {
    burrow_admin_proto::updates::LatestRelease {
        version: Some(NEW.into()),
        ..Default::default()
    }
}

async fn start(hand: &mut Hand, w: &World) -> Admin {
    burrow::admin::adapters::use_latest(latest);
    hand.supervise();
    let mut a = connect(w, "ops", &w.ops, 1, 1).await.unwrap();
    a.call("subscribe", json!({"status_ms": 200}))
        .await
        .unwrap();
    until(&mut a, "report of the old version", |c| {
        c["build"]["version"] == OLD
    })
    .await;
    a
}

async fn ask(a: &mut Admin) -> Value {
    a.call(
        "update_clients",
        json!({"clients": ["home"], "version": NEW}),
    )
    .await
    .unwrap()
}

fn same(path: &Path, bytes: &[u8]) -> bool {
    std::fs::read(path).is_ok_and(|b| b == bytes)
}

#[tokio::test]
async fn a_client_started_by_hand_updates_itself_and_reconnects_with_the_new_version() {
    let w = world().await;
    let mut hand = Hand::new(&w);
    let new = build(&[("VERSION", NEW)]);
    hand.releases.publish(NEW, &new, false);
    let mut a = start(&mut hand, &w).await;
    assert_eq!(ask(&mut a).await["asked"], json!(["home"]));
    let home = until(&mut a, "the new version", |c| {
        c["update"]["state"] == "done"
    })
    .await;
    assert_eq!(home["build"]["version"], NEW);
    assert_eq!(
        home["update"]["by"].as_str().unwrap().split(' ').next(),
        Some("ops")
    );
    assert!(same(&hand.binary, &new));
    assert!(same(
        &hand.binary.with_extension("previous"),
        &build(&[("VERSION", OLD)])
    ));
    assert_eq!(hand.record()["outcome"], "done");
    let older = a
        .call(
            "update_clients",
            json!({"clients": ["home"], "version": OLD}),
        )
        .await
        .unwrap_err();
    assert!(
        older.message.contains("only to the latest release"),
        "{older:?}"
    );
    let again = ask(&mut a).await;
    assert!(
        again["skipped"][0]["why"]
            .as_str()
            .unwrap()
            .contains("already runs"),
        "{again}"
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_download_that_does_not_match_its_checksum_changes_nothing() {
    let w = world().await;
    let mut hand = Hand::new(&w);
    hand.releases
        .publish(NEW, &build(&[("VERSION", NEW)]), true);
    let mut a = start(&mut hand, &w).await;
    ask(&mut a).await;
    let home = until(&mut a, "a failure", |c| c["update"]["state"] == "failed").await;
    assert!(
        home["update"]["detail"]
            .as_str()
            .unwrap()
            .contains("SHA-256"),
        "{home}"
    );
    assert!(same(&hand.binary, &build(&[("VERSION", OLD)])));
    assert_eq!(home["build"]["version"], OLD);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_new_version_that_does_not_connect_is_rolled_back() {
    let w = world().await;
    let mut hand = Hand::new(&w);
    hand.releases.publish(
        NEW,
        &build(&[("VERSION", NEW), ("BREAK", "connect")]),
        false,
    );
    let mut a = start(&mut hand, &w).await;
    ask(&mut a).await;
    let home = until(&mut a, "a rollback", |c| {
        c["update"]["state"] == "rolled-back"
    })
    .await;
    assert!(
        home["update"]["detail"]
            .as_str()
            .unwrap()
            .contains("did not connect"),
        "{home}"
    );
    assert_eq!(home["build"]["version"], OLD);
    assert!(same(&hand.binary, &build(&[("VERSION", OLD)])));
    assert_eq!(hand.record()["outcome"], "rolled-back");
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_new_version_that_keeps_crashing_is_rolled_back_by_the_next_start() {
    let w = world().await;
    let mut hand = Hand::new(&w);
    hand.releases
        .publish(NEW, &build(&[("VERSION", NEW), ("BREAK", "start")]), false);
    let mut a = start(&mut hand, &w).await;
    ask(&mut a).await;
    let home = until(&mut a, "a rollback", |c| {
        c["update"]["state"] == "rolled-back"
    })
    .await;
    assert!(
        home["update"]["detail"]
            .as_str()
            .unwrap()
            .contains("stopped 2 times"),
        "{home}"
    );
    assert!(same(&hand.binary, &build(&[("VERSION", OLD)])));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_release_that_is_not_published_is_refused_by_the_client() {
    let w = world().await;
    let mut hand = Hand::new(&w);
    let mut a = start(&mut hand, &w).await;
    ask(&mut a).await;
    let home = until(&mut a, "a failure", |c| c["update"]["state"] == "failed").await;
    assert!(
        home["update"]["detail"]
            .as_str()
            .unwrap()
            .contains("no burrow release was found"),
        "{home}"
    );
    assert!(same(&hand.binary, &build(&[("VERSION", OLD)])));
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_release_that_does_not_accept_the_config_changes_nothing() {
    let w = world().await;
    let mut hand = Hand::new(&w);
    hand.releases
        .publish(NEW, &build(&[("VERSION", NEW), ("BREAK", "config")]), false);
    let mut a = start(&mut hand, &w).await;
    ask(&mut a).await;
    let home = until(&mut a, "a failure", |c| c["update"]["state"] == "failed").await;
    assert!(
        home["update"]["detail"]
            .as_str()
            .unwrap()
            .contains("does not accept"),
        "{home}"
    );
    assert!(same(&hand.binary, &build(&[("VERSION", OLD)])));
    let _ = w.stop.send(true);
}
