mod common;

use std::io::{Read, Write};
use std::path::Path;

use common::releases::Releases;
use common::servers::{server_config, start_with};

fn entry(instances: &Path, pid: u32) -> (u16, String) {
    let file = instances.join(format!("{pid}.status"));
    let entry: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    (
        entry["port"].as_u64().unwrap() as u16,
        entry["token"].as_str().unwrap().to_string(),
    )
}

fn raw(port: u16, head: &str, body: &str) -> String {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(
        format!(
            "{head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn the_update_route_needs_the_token_and_takes_only_a_post() {
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    let config = server_config(dir.path(), "server.yaml");
    let releases = Releases::start();
    releases.publish("9.0.0", b"new", false);
    let server = start_with(
        &instances,
        &config,
        &[("BURROW_UPDATE_URL", &releases.base)],
    );
    let (port, token) = entry(&instances, server.0.id());
    let auth = format!("Authorization: Bearer {token}");
    let post = |auth: &str, body: &str| {
        raw(
            port,
            &format!("POST /clients/update HTTP/1.1\r\nHost: x\r\n{auth}"),
            body,
        )
    };
    let none = r#"{"clients":["nobody"],"dry_run":true}"#;
    assert!(post("X-No: 1", none).starts_with("HTTP/1.1 401"));
    assert!(post("Authorization: Bearer nope", none).starts_with("HTTP/1.1 401"));
    let answer = post(&auth, none);
    assert!(
        answer.starts_with("HTTP/1.1 200") || answer.starts_with("HTTP/1.1 400"),
        "{answer}"
    );
    assert!(
        answer.contains("there is no client with this name") || answer.contains("latest release"),
        "{answer}"
    );
    assert!(post(&auth, "{not json").starts_with("HTTP/1.1 400"));
    let other = |method: &str, path: &str| {
        raw(
            port,
            &format!("{method} {path} HTTP/1.1\r\nHost: x\r\n{auth}"),
            "",
        )
    };
    assert!(other("GET", "/clients/update").starts_with("HTTP/1.1 405"));
    assert!(other("PUT", "/clients/update").starts_with("HTTP/1.1 405"));
    assert!(other("POST", "/status").starts_with("HTTP/1.1 405"));
    assert!(other("POST", "/config").starts_with("HTTP/1.1 404"));
    assert!(other("GET", "/config").starts_with("HTTP/1.1 404"));
    assert!(other("GET", "/status").starts_with("HTTP/1.1 200"));
}

#[cfg(feature = "admin-test-hooks")]
#[test]
fn burrow_clients_updates_a_client_through_the_same_job_the_admin_starts() {
    use burrow::metrics::Status;
    use burrow::wire::secret::Secret;
    use common::hand::{Hand, NEW, OLD, build};
    use common::servers::Running;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const BIN: &str = env!("CARGO_BIN_EXE_burrow");
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    let port = common::free_port();
    let secret = Secret::generate();
    let config = dir.path().join("server.yaml");
    burrow::base::fs::write_private(
        &config,
        &format!(
            "listen: 127.0.0.1:{port}\nclients:\n  home:\n    secret: {}\n",
            secret.encode()
        ),
    )
    .unwrap();
    let mut hand = Hand::new(&format!("127.0.0.1:{port}"), &secret);
    hand.releases
        .publish(NEW, &build(&[("VERSION", NEW)]), false);
    let _server = Running(
        Command::new(BIN)
            .args(["server", "-c"])
            .arg(&config)
            .env("BURROW_INSTANCE_DIR", &instances)
            .env("BURROW_UPDATE_URL", &hand.releases.base)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    hand.supervise();
    let home = || -> Option<burrow::metrics::ClientStatus> {
        let o = Command::new(BIN)
            .args(["status", "--json"])
            .env("BURROW_INSTANCE_DIR", &instances)
            .output()
            .ok()?;
        let list: Vec<serde_json::Value> = serde_json::from_slice(&o.stdout).ok()?;
        let st: Status = serde_json::from_value(list.first()?["status"].clone()).ok()?;
        st.clients.into_iter().find(|c| c.name == "home")
    };
    let start = Instant::now();
    while !home().is_some_and(|c| c.build.is_some_and(|b| b.version == OLD)) {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "no report of {OLD}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let o = Command::new(BIN)
        .args(["clients", "update", "home", "--yes"])
        .env("BURROW_INSTANCE_DIR", &instances)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let shown = common::text(&o);
    assert_eq!(o.status.code(), Some(0), "{shown}");
    assert!(shown.contains(&format!("update home to {NEW}")), "{shown}");
    assert!(
        shown.contains(&format!("home: {OLD} → {NEW} updated")),
        "{shown}"
    );
    let after = home().unwrap();
    assert_eq!(after.build.unwrap().version, NEW);
    let job = after.update.unwrap();
    assert_eq!(job.by, "burrow clients on the server");
    assert_eq!(job.state, burrow_admin_proto::updates::UpdateState::Done);
}
