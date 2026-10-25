use serde_json::{Value, json};

use super::common::admin::world;
use super::{ai, visitor};

const READ: [&str; 4] = [
    "describe_settings",
    "get_config",
    "get_status",
    "list_versions",
];

fn names(extra: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = READ.iter().chain(extra).map(|s| s.to_string()).collect();
    v.sort();
    v
}

fn set_target(port: u16) -> Value {
    json!({ "ops": [{ "op": "set", "path": ["services", "web", "target"], "value": port }] })
}

#[tokio::test]
async fn a_read_admin_gets_only_the_read_tools_even_with_allow_write() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    for args in [&[][..], &["--allow-write"][..]] {
        let a = ai(&v, args).await;
        assert_eq!(a.tools().await, names(&[]), "{args:?}");
        let (failed, _) = a.call("check_change", set_target(3001)).await;
        assert!(failed, "a tool that is not listed cannot be called");
        a.stop().await;
    }
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_write_admin_checks_changes_but_applies_them_only_with_allow_write() {
    let w = world().await;
    let v = visitor(&w, "ops", &w.ops);
    let a = ai(&v, &[]).await;
    assert_eq!(a.tools().await, names(&["check_change"]));
    let plan = a.ok("check_change", set_target(3001)).await;
    assert_eq!(plan["can_apply"], true, "{plan}");
    let (failed, _) = a
        .call("apply_change", json!({ "plan": plan["plan"] }))
        .await;
    assert!(failed, "apply_change is not offered without --allow-write");
    a.stop().await;
    let a = ai(&v, &["--allow-write"]).await;
    assert_eq!(
        a.tools().await,
        names(&["apply_change", "check_change", "revert_to_version"])
    );
    a.stop().await;
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_read_tools_answer_from_the_server() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let a = ai(&v, &[]).await;
    let status = a.ok("get_status", json!({})).await;
    assert!(
        status.is_object() && !status.as_object().unwrap().is_empty(),
        "{status}"
    );
    let config = a.ok("get_config", json!({})).await;
    assert!(
        config["rev"].as_str().is_some_and(|r| !r.is_empty()),
        "{config}"
    );
    assert_eq!(
        config["values"]["services"]["web"]["target"], 3000,
        "{config}"
    );
    let secret = &config["values"]["visitors"]["ops"]["secret"];
    assert!(
        !secret.is_null(),
        "the secret shows as its fingerprint: {config}"
    );
    let web = a
        .ok("describe_settings", json!({ "path": ["services"] }))
        .await;
    assert!(web["setting"].is_object(), "{web}");
    let (failed, why) = a
        .call("describe_settings", json!({ "path": ["no-such-key"] }))
        .await;
    assert!(failed && why["error"] == "unknown-path", "{why}");
    let versions = a.ok("list_versions", json!({})).await;
    assert!(versions["entries"].is_array(), "{versions}");
    a.stop().await;
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_checked_plan_is_applied_and_reverted_through_the_same_tool() {
    let w = world().await;
    let v = visitor(&w, "ops", &w.ops);
    let a = ai(&v, &["--allow-write"]).await;
    let plan = a.ok("check_change", set_target(3001)).await;
    assert!(
        plan["diff"]["unified"].as_str().unwrap().contains("3001"),
        "{plan}"
    );
    let applied = a
        .ok(
            "apply_change",
            json!({ "plan": plan["plan"], "confirm": plan["confirm"] }),
        )
        .await;
    assert_eq!(applied["rev"], plan["result_rev"], "{applied}");
    let text = std::fs::read_to_string(&w.path).unwrap();
    assert!(text.contains("target: 3001"), "{text}");
    let (failed, why) = a
        .call("apply_change", json!({ "plan": plan["plan"] }))
        .await;
    assert!(
        failed && why["error"] == "unknown-plan",
        "an applied plan is used up: {why}"
    );
    let versions = a.ok("list_versions", json!({ "limit": 5 })).await;
    let before = versions["entries"][0]["base"].clone();
    assert!(before.is_string(), "{versions}");
    let back = a
        .ok("revert_to_version", json!({ "version": before }))
        .await;
    assert_eq!(back["can_apply"], true, "{back}");
    a.ok(
        "apply_change",
        json!({ "plan": back["plan"], "confirm": back["confirm"] }),
    )
    .await;
    let text = std::fs::read_to_string(&w.path).unwrap();
    assert!(text.contains("target: 3000"), "{text}");
    a.stop().await;
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_plan_is_refused_after_the_config_changed() {
    let w = world().await;
    let v = visitor(&w, "ops", &w.ops);
    let a = ai(&v, &["--allow-write"]).await;
    let first = a.ok("check_change", set_target(3001)).await;
    let second = a.ok("check_change", set_target(3002)).await;
    a.ok("apply_change", json!({ "plan": second["plan"] }))
        .await;
    let (failed, why) = a
        .call("apply_change", json!({ "plan": first["plan"] }))
        .await;
    assert!(failed && why["error"] == "conflict", "{why}");
    let text = std::fs::read_to_string(&w.path).unwrap();
    assert!(text.contains("target: 3002"), "{text}");
    a.stop().await;
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn a_visitor_without_admin_rights_ends_with_the_reason() {
    let w = world().await;
    let v = visitor(&w, "plain", &w.plain);
    let out = v
        .command(&[])
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("has no admin rights"), "{stderr}");
    assert!(out.stdout.is_empty(), "nothing but MCP goes to stdout");
    let _ = w.stop.send(true);
}

#[cfg(unix)]
#[tokio::test]
async fn started_by_hand_it_explains_itself_on_stderr_only() {
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let name = ptsname(&master, Vec::new()).unwrap();
    let terminal = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(name.to_str().unwrap())
        .unwrap();
    let mut child = v
        .command(&[])
        .stdin(terminal)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut seen = String::new();
    let found = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while let Ok(Some(l)) = lines.next_line().await {
            seen.push_str(&l);
            seen.push('\n');
            if l.trim_start().starts_with("Cursor:") {
                return true;
            }
        }
        false
    })
    .await;
    assert!(matches!(found, Ok(true)), "{seen}");
    assert!(
        seen.contains("meant to be started by your AI app"),
        "{seen}"
    );
    assert!(
        seen.contains("--print-config claude") && !seen.contains("--http"),
        "{seen}"
    );
    child.kill().await.unwrap();
    let mut out = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut out)
        .await
        .unwrap();
    assert!(out.is_empty(), "stdout: {}", String::from_utf8_lossy(&out));
    drop(master);
    let _ = w.stop.send(true);
}
