use std::time::{Duration, Instant};

use serde_json::json;

use super::app::{self, App, STOP};
use super::common::admin::world;
use super::{server_line, visitor, visitor_to};

const READ: [&str; 5] = [
    "describe_settings",
    "get_config",
    "get_status",
    "list_versions",
    "validate_config",
];

fn closed_port() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    server_line(addr)
}

async fn silent_server() -> std::net::SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = l.accept().await {
            held.push(s);
        }
    });
    addr
}

#[derive(Clone, Copy, Debug)]
enum Stop {
    StdinClosed,
    #[cfg(unix)]
    Interrupt,
    #[cfg(unix)]
    Terminate,
}

fn every_stop() -> Vec<Stop> {
    #[cfg(unix)]
    return vec![Stop::StdinClosed, Stop::Interrupt, Stop::Terminate];
    #[cfg(not(unix))]
    return vec![Stop::StdinClosed];
}

async fn stop(a: &mut App, how: Stop) {
    match how {
        Stop::StdinClosed => a.close_stdin(),
        #[cfg(unix)]
        Stop::Interrupt => a.signal(rustix::process::Signal::INT),
        #[cfg(unix)]
        Stop::Terminate => a.signal(rustix::process::Signal::TERM),
    }
    a.exits_within(STOP).await;
    a.rest_of_stdout().await;
}

#[tokio::test]
async fn an_app_started_elsewhere_gets_the_handshake_the_tools_and_a_read_tool() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&v.config, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let mut a = app::mcp(&v.config, &[], &v.sessions());
    let info = a.ready().await;
    assert_eq!(info["serverInfo"]["name"], "burrow", "{info}");
    assert_eq!(a.tools().await, READ);
    let (failed, status) = a.call("get_status", json!({})).await;
    assert!(!failed, "{status}");
    assert_eq!(status["mcp_link"]["state"], "up", "{status}");
    stop(&mut a, Stop::StdinClosed).await;
    v.assert_no_secret(&a.lines.join("\n"));
    #[cfg(unix)]
    assert!(
        a.stderr().contains("readable by every user"),
        "the mode warning goes to stderr: {}",
        a.stderr()
    );
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn an_unreachable_server_still_answers_the_handshake_and_each_call_says_where_it_failed() {
    let w = world().await;
    let v = visitor_to(&w, "ops", &w.ops, &closed_port());
    let mut a = app::mcp(&v.config, &[], &v.sessions());
    let started = Instant::now();
    a.ready().await;
    assert!(started.elapsed() < Duration::from_secs(5));
    for tool in ["get_status", "get_config"] {
        let (failed, why) = a.call(tool, json!({})).await;
        assert!(failed, "{why}");
        let message = why["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with("burrow mcp on this computer could not connect to"),
            "{why}"
        );
        assert!(message.contains("connection refused"), "{why}");
        let hint = why["hint"].as_str().unwrap_or_default();
        assert!(hint.contains("check this computer's network"), "{why}");
        assert!(hint.contains("tries again on the next call"), "{why}");
        assert_eq!(why["mcp_link"]["state"], "down", "{why}");
    }
    let (failed, checked) = a
        .call("validate_config", json!({ "config": "listen: nowhere\n" }))
        .await;
    assert!(!failed && checked["valid"] == false, "{checked}");
    assert_eq!(checked["checked_by"], "local", "{checked}");
    let note = checked["note"].as_str().unwrap_or_default();
    assert!(note.contains("could not be reached"), "{checked}");
    assert!(note.contains("on this computer"), "{checked}");
    stop(&mut a, Stop::StdinClosed).await;
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_server_name_that_does_not_resolve_is_named_as_a_dns_failure() {
    let w = world().await;
    let name = if super::common::ws_mode() {
        "ws://burrow-mcp-test.invalid:7835/burrow"
    } else {
        "burrow-mcp-test.invalid:7835"
    };
    let v = visitor_to(&w, "ops", &w.ops, name);
    let mut a = app::mcp(&v.config, &[], &v.sessions());
    a.ready().await;
    let (failed, why) = a.call("get_status", json!({})).await;
    assert!(failed, "{why}");
    let message = why["message"].as_str().unwrap_or_default();
    assert!(message.contains("on this computer"), "{why}");
    assert!(
        message.contains("could not look up burrow-mcp-test.invalid"),
        "{why}"
    );
    assert!(message.contains("DNS"), "{why}");
    stop(&mut a, Stop::StdinClosed).await;
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_link_comes_back_after_the_network_was_cut() {
    let w = world().await;
    let relay = app::relay(w.addr).await;
    let v = visitor_to(&w, "viewer", &w.viewer, &server_line(relay.addr));
    let mut a = app::mcp(&v.config, &[], &v.sessions());
    a.ready().await;
    let (failed, first) = a.call("get_status", json!({})).await;
    assert!(!failed, "{first}");
    relay.cut();
    let (failed, why) = a.call("get_status", json!({})).await;
    assert!(failed, "{why}");
    assert!(
        why["message"]
            .as_str()
            .unwrap_or_default()
            .contains("on this computer"),
        "{why}"
    );
    relay.restore();
    let until = Instant::now() + Duration::from_secs(40);
    loop {
        let (failed, status) = a.call("get_status", json!({})).await;
        if !failed {
            assert_eq!(status["mcp_link"]["state"], "up", "{status}");
            break;
        }
        assert!(Instant::now() < until, "the link never came back: {status}");
    }
    stop(&mut a, Stop::StdinClosed).await;
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn it_stops_within_a_second_before_the_handshake() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    for how in every_stop() {
        let mut a = app::mcp(&v.config, &[], &v.sessions());
        tokio::time::sleep(Duration::from_secs(1)).await;
        stop(&mut a, how).await;
        assert!(a.lines.is_empty(), "{how:?}: {:?}", a.lines);
    }
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn it_stops_within_a_second_after_the_handshake() {
    let w = world().await;
    let v = visitor(&w, "ops", &w.ops);
    for how in every_stop() {
        let mut a = app::mcp(&v.config, &[], &v.sessions());
        a.ready().await;
        assert!(a.tools().await.len() > READ.len());
        stop(&mut a, how).await;
    }
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn it_stops_within_a_second_while_a_call_waits_for_a_silent_server() {
    let w = world().await;
    let silent = silent_server().await;
    let v = visitor_to(&w, "ops", &w.ops, &server_line(silent));
    for how in every_stop() {
        let mut a = app::mcp(&v.config, &[], &v.sessions());
        a.ready().await;
        a.send(json!({
            "jsonrpc": "2.0", "id": 99, "method": "tools/call",
            "params": { "name": "get_status", "arguments": {} }
        }))
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop(&mut a, how).await;
    }
    let _ = w.stop.send(true);
}
