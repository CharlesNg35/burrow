use std::process::Stdio;

use clap::Parser;
use rmcp::ServiceExt;
use rmcp::transport::TokioChildProcess;
use serde_json::Value;

use super::common::admin::world;
use super::http::{list_with, start_on};
use super::{Visitor, visitor};

const MCP_MD: &str = include_str!("../../docs/mcp.md");

async fn printed(v: &Visitor, args: &[&str]) -> (String, String) {
    let out = v
        .command(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn blocks(lang: &str) -> Vec<String> {
    let fence = format!("```{lang}\n");
    MCP_MD
        .replace("\r\n", "\n")
        .split(&fence)
        .skip(1)
        .map(|b| b.split("```").next().unwrap().to_string())
        .collect()
}

fn command_in(printed: &str) -> Vec<String> {
    let (comment, command) = printed
        .trim()
        .split_once('\n')
        .expect("a comment line first");
    assert!(comment.starts_with("# Claude Code"), "{printed}");
    shlex::split(command).unwrap()
}

fn burrow_args(words: &[String]) -> Vec<String> {
    let at = words
        .iter()
        .position(|w| w == "--")
        .expect("a -- before the command");
    words[at + 1..].to_vec()
}

#[tokio::test]
async fn the_printed_stdio_setup_starts_a_working_server() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let (codex, _) = printed(&v, &["--print-config", "codex"]).await;
    assert!(codex.starts_with("# ~/.codex/config.toml\n"), "{codex}");
    let t: toml::Value = toml::from_str(&codex).unwrap();
    let entry = &t["mcp_servers"]["burrow"];
    assert_eq!(entry["command"].as_str(), Some("burrow"));
    let args: Vec<String> = entry["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect();
    assert!(std::path::Path::new(&args[2]).is_absolute(), "{args:?}");
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_burrow"));
    cmd.args(&args)
        .env("BURROW_ADMIN_SESSION_DIR", v.sessions())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    assert_eq!(client.list_all_tools().await.unwrap().len(), 4);
    let _ = client.cancel().await;
    let (cursor, header) = printed(&v, &["--print-config", "cursor"]).await;
    assert_eq!(
        header.lines().last(),
        Some("// ~/.cursor/mcp.json"),
        "{header}"
    );
    let c: Value = serde_json::from_str(&cursor).unwrap();
    assert_eq!(
        c["mcpServers"]["burrow"]["args"].as_array().unwrap().len(),
        3
    );
    let (claude, _) = printed(&v, &["--print-config", "claude"]).await;
    let words = command_in(&claude);
    assert_eq!(burrow_args(&words)[..2], ["burrow", "mcp"]);
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn the_printed_http_setups_connect_with_their_url_and_token() {
    let w = world().await;
    let v = visitor(&w, "viewer", &w.viewer);
    let port = super::common::free_port();
    let p = port.to_string();
    let h = start_on(&v, &p, &[]).await;
    let (cursor, _) = printed(&v, &["--http", "--port", &p, "--print-config", "cursor"]).await;
    let c: Value = serde_json::from_str(&cursor).unwrap();
    let entry = &c["mcpServers"]["burrow"];
    let auth = entry["headers"]["Authorization"].as_str().unwrap();
    assert_eq!(
        entry["url"].as_str(),
        Some(format!("http://127.0.0.1:{port}/mcp").as_str())
    );
    assert_eq!(list_with(port, auth).await, 200);
    let (claude, _) = printed(&v, &["--http", "--port", &p, "--print-config", "claude"]).await;
    let words = command_in(&claude);
    let header = &words[words.iter().position(|x| x == "--header").unwrap() + 1];
    let auth = header.strip_prefix("Authorization: ").unwrap();
    assert!(
        words.contains(&format!("http://127.0.0.1:{port}/mcp")),
        "{claude}"
    );
    assert_eq!(list_with(port, auth).await, 200);
    let (codex, after) = printed(&v, &["--http", "--port", &p, "--print-config", "codex"]).await;
    let t: toml::Value = toml::from_str(&codex).unwrap();
    assert_eq!(
        t["mcp_servers"]["burrow"]["bearer_token_env_var"].as_str(),
        Some("BURROW_MCP_TOKEN")
    );
    let token = after
        .lines()
        .find_map(|l| l.trim().strip_prefix("export BURROW_MCP_TOKEN="))
        .or_else(|| cfg!(windows).then_some(""))
        .unwrap();
    if !cfg!(windows) {
        assert_eq!(token, h.token);
        assert_eq!(list_with(port, &format!("Bearer {token}")).await, 200);
    }
    let _ = w.stop.send(true);
}

#[test]
fn an_unknown_app_lists_the_three() {
    let e = burrow::app::cli::Cli::try_parse_from(["burrow", "mcp", "--print-config", "vim"])
        .unwrap_err()
        .to_string();
    for app in ["claude", "codex", "cursor"] {
        assert!(e.contains(app), "{e}");
    }
}

#[test]
fn every_setup_in_mcp_md_is_valid() {
    let json = blocks("json");
    assert!(json.len() >= 3, "{json:?}");
    for b in &json {
        let (first, rest) = b.split_once('\n').unwrap();
        assert!(first.starts_with("// "), "{b}");
        let v: Value = serde_json::from_str(rest).unwrap_or_else(|e| panic!("{e}: {b}"));
        let entry = &v["mcpServers"]["burrow"];
        if let Some(args) = entry["args"].as_array() {
            let mut line = vec!["burrow".to_string()];
            line.extend(args.iter().map(|a| a.as_str().unwrap().to_string()));
            burrow::app::cli::Cli::try_parse_from(&line).unwrap_or_else(|e| panic!("{e}: {b}"));
        } else {
            assert!(entry["url"].as_str().unwrap().ends_with(":7838/mcp"), "{b}");
        }
    }
    let toml_blocks = blocks("toml");
    assert!(toml_blocks.len() >= 2, "{toml_blocks:?}");
    for b in &toml_blocks {
        assert!(b.starts_with("# "), "{b}");
        let v: toml::Value = toml::from_str(b).unwrap_or_else(|e| panic!("{e}: {b}"));
        assert!(v["mcp_servers"]["burrow"].is_table(), "{b}");
    }
    let mut commands = 0;
    for b in blocks("sh") {
        assert!(b.starts_with("# "), "{b}");
        for line in b
            .lines()
            .filter(|l| l.starts_with("claude mcp add") || l.starts_with("burrow mcp"))
        {
            let words = shlex::split(line.split("  #").next().unwrap()).unwrap();
            let args = if words[0] == "claude" {
                if !words.contains(&"--".to_string()) {
                    continue;
                }
                burrow_args(&words)
            } else {
                words
            };
            burrow::app::cli::Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{e}: {line}"));
            commands += 1;
        }
    }
    assert!(commands >= 3, "{commands}");
}
