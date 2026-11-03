use std::process::Stdio;

use serde_json::{Value, json};

use super::app::{self, STOP};
use super::common::admin::world;
use super::visitor;

const WRITE_TOOLS: [&str; 4] = [
    "apply_change",
    "check_change",
    "revert_to_version",
    "update_clients",
];

fn startup_error(v: &Value) -> String {
    let e = &v["error"];
    assert!(e.is_object(), "the app gets an MCP error: {v}");
    e["message"].as_str().unwrap_or_default().to_string()
}

fn log_named_in(message: &str) -> std::path::PathBuf {
    let at = message
        .find("details are in ")
        .unwrap_or_else(|| panic!("the error names its log: {message}"));
    let rest = &message[at + "details are in ".len()..];
    std::path::PathBuf::from(rest.trim_end_matches('.'))
}

#[tokio::test]
async fn a_missing_config_is_an_error_the_app_can_see_and_a_log_keeps_it() {
    let state = tempfile::tempdir().unwrap();
    let missing = state.path().join("nowhere").join("visitor.yaml");
    let mut a = app::mcp(&missing, &[], state.path());
    let v = a.initialize().await;
    let message = startup_error(&v);
    assert!(
        message.starts_with("burrow mcp could not start:"),
        "{message}"
    );
    assert!(message.contains("visitor.yaml"), "{message}");
    let log = log_named_in(&message);
    let kept = std::fs::read_to_string(&log).unwrap();
    assert!(kept.contains("visitor.yaml"), "{kept}");
    a.close_stdin();
    let status = a.exits_within(STOP).await;
    assert!(!status.success());
    a.rest_of_stdout().await;
}

#[tokio::test]
async fn no_config_in_the_app_s_folder_is_an_error_the_app_can_see() {
    let state = tempfile::tempdir().unwrap();
    let mut a = app::launch(&["mcp"], state.path());
    let v = a.initialize().await;
    let message = startup_error(&v);
    assert!(
        message.starts_with("burrow mcp could not start:"),
        "{message}"
    );
    assert!(
        message.contains(" -c "),
        "the error says to pass -c: {message}"
    );
    a.close_stdin();
    assert!(!a.exits_within(STOP).await.success());
}

#[tokio::test]
async fn the_whole_command_in_one_argument_is_explained() {
    let state = tempfile::tempdir().unwrap();
    let config = state.path().join("visitor.yaml");
    let whole = format!("mcp -c {}", config.display());
    let flag = format!("-c {}", config.display());
    for args in [vec![whole.as_str()], vec!["mcp", flag.as_str()]] {
        let mut a = app::launch(&args, state.path());
        let v = a.initialize().await;
        let message = startup_error(&v);
        assert!(message.contains("as one argument"), "{args:?}: {message}");
        assert!(message.contains("its own line"), "{args:?}: {message}");
        a.close_stdin();
        assert_eq!(a.exits_within(STOP).await.code(), Some(2), "{args:?}");
    }
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_burrow"))
        .arg(&whole)
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("its own line"), "{stderr}");
    assert!(out.stdout.is_empty());
}

#[tokio::test]
async fn stdin_closed_before_any_app_spoke_is_said_plainly() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let out = v
        .command(&[])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("initialize request"), "{stderr}");
    assert!(
        stderr.contains("no AI app spoke to it"),
        "the plain wording: {stderr}"
    );
    assert!(out.stdout.is_empty());
    let _ = w.stop.send(true);
}

fn set_target() -> Value {
    json!({ "ops": [{ "op": "set", "path": ["services", "web", "target"], "value": 3001 }] })
}

#[tokio::test]
async fn the_instructions_and_the_tools_agree_in_each_mode() {
    let w = world().await;
    let cases: [(&str, &burrow::wire::secret::Secret, &[&str], &str); 3] = [
        ("viewer", &w.viewer, &[], "read-only"),
        ("ops", &w.ops, &[], "check-only"),
        ("ops", &w.ops, &["--allow-write"], "write"),
    ];
    for (name, secret, args, mode) in cases {
        let v = visitor(&w, name, secret);
        let mut a = app::mcp(&v.config, args, &v.sessions());
        let info = a.ready().await;
        let text = info["instructions"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let tools = a.tools().await;
        for t in WRITE_TOOLS {
            if text.contains(t) {
                assert!(tools.iter().any(|x| x == t), "{mode}: names {t}: {text}");
            }
        }
        match mode {
            "read-only" => {
                assert!(text.contains("read-only"), "{text}");
                assert!(!tools.iter().any(|t| t == "check_change"), "{tools:?}");
            }
            "check-only" => {
                assert!(text.contains("--allow-write"), "{text}");
                assert!(text.contains("cannot be applied"), "{text}");
                assert_eq!(
                    tools
                        .iter()
                        .filter(|t| WRITE_TOOLS.contains(&t.as_str()))
                        .count(),
                    1
                );
                let (failed, plan) = a.call("check_change", set_target()).await;
                assert!(!failed, "{plan}");
                let note = plan["apply"].as_str().unwrap_or_default();
                assert!(note.contains("--allow-write"), "{plan}");
            }
            _ => {
                assert!(text.contains("apply_change"), "{text}");
                assert!(WRITE_TOOLS.iter().all(|t| tools.iter().any(|x| x == t)));
                let (failed, plan) = a.call("check_change", set_target()).await;
                assert!(!failed && plan.get("apply").is_none(), "{plan}");
            }
        }
        a.close_stdin();
        a.exits_within(STOP).await;
    }
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn print_config_prints_one_standard_config_with_full_paths() {
    let w = world().await;
    let v = visitor(&w, "ops", &w.ops);
    let elsewhere = tempfile::tempdir().unwrap();
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["mcp", "-c"])
        .arg(&v.config)
        .args(["--allow-write", "--print-config"])
        .current_dir(elsewhere.path())
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let json: Value = serde_json::from_slice(&out.stdout).expect("stdout is the JSON alone");
    let entry = &json["mcpServers"]["burrow"];
    let command = entry["command"].as_str().unwrap();
    assert!(std::path::Path::new(command).is_absolute(), "{json}");
    let config = dunce::canonicalize(&v.config)
        .unwrap()
        .display()
        .to_string();
    assert_eq!(entry["args"], json!(["mcp", "-c", config, "--allow-write"]));
    let form = format!("Arguments, one per line:\n  mcp\n  -c\n  {config}\n  --allow-write\n");
    assert!(stderr.contains(&form), "{stderr}");
    assert!(
        stderr.contains(&format!("Command:\n  {command}\n")),
        "{stderr}"
    );
    let codex = tokio::process::Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["mcp", "-c"])
        .arg(&v.config)
        .args(["--allow-write", "--print-config", "codex"])
        .current_dir(elsewhere.path())
        .output()
        .await
        .unwrap();
    assert!(codex.status.success());
    let t: toml::Value = toml::from_str(&String::from_utf8_lossy(&codex.stdout)).unwrap();
    let same = &t["mcp_servers"]["burrow"];
    assert_eq!(
        same["command"].as_str(),
        Some(command),
        "one builder, same values"
    );
    assert_eq!(same["args"].as_array().unwrap().len(), 4);
    assert_eq!(same["args"][2].as_str(), Some(config.as_str()));
    let _ = w.stop.send(true);
}
