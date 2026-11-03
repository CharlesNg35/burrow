use std::process::{Output, Stdio};

use super::common::admin::world;
use super::{Visitor, visitor, visitor_to};

async fn check(v: &Visitor, extra: &[&str]) -> (Output, String, String) {
    let elsewhere = tempfile::tempdir().unwrap();
    let out = v
        .command(&["--check"])
        .args(extra)
        .current_dir(elsewhere.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out, stdout, stderr)
}

#[tokio::test]
async fn check_reports_what_an_app_would_get() {
    let w = world().await;
    let ops = visitor(&w, "ops", &w.ops);
    let (out, text, err) = check(&ops, &[]).await;
    assert!(out.status.success(), "{text}{err}");
    assert!(text.contains("config:"), "{text}");
    assert!(text.contains("(visitor \"ops\")"), "{text}");
    assert!(text.contains("server:  reached"), "{text}");
    assert!(text.contains("admin:   write"), "{text}");
    assert!(text.contains("writes:  off"), "{text}");
    assert!(text.contains("--allow-write"), "{text}");
    assert!(text.contains("tools:   6 "), "{text}");
    let (out, text, err) = check(&ops, &["--allow-write"]).await;
    assert!(out.status.success(), "{text}{err}");
    assert!(text.contains("writes:  allowed"), "{text}");
    assert!(text.contains("tools:   9 "), "{text}");
    let viewer = visitor(&w, "viewer", &w.viewer);
    let (out, text, err) = check(&viewer, &["--allow-write"]).await;
    assert!(out.status.success(), "{text}{err}");
    assert!(text.contains("admin:   read"), "{text}");
    assert!(text.contains("tools:   5 "), "{text}");
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn check_fails_with_the_reason_when_the_app_would_get_nothing() {
    let w = world().await;
    let plain = visitor(&w, "plain", &w.plain);
    let (out, text, err) = check(&plain, &[]).await;
    assert_eq!(out.status.code(), Some(1), "{text}{err}");
    assert!(err.contains("has no admin rights"), "{text}{err}");
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = super::server_line(l.local_addr().unwrap());
    drop(l);
    let away = visitor_to(&w, "ops", &w.ops, &closed);
    let (out, text, err) = check(&away, &[]).await;
    assert_eq!(out.status.code(), Some(1), "{text}{err}");
    assert!(err.contains("could not connect to"), "{text}{err}");
    let _ = w.stop.send(true);
}
