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
