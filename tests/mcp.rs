mod common;
#[path = "mcp/docs.rs"]
mod docs;
#[path = "mcp/http.rs"]
mod http;
#[path = "mcp/limits.rs"]
mod limits;
#[path = "mcp/stdio.rs"]
mod stdio;

use std::path::PathBuf;

use burrow::wire::secret::Secret;
use common::admin::World;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;

pub struct Visitor {
    pub dir: tempfile::TempDir,
    pub config: PathBuf,
    secrets: Vec<String>,
}

impl Visitor {
    pub fn sessions(&self) -> PathBuf {
        self.dir.path().join("sessions")
    }

    pub fn command(&self, args: &[&str]) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_burrow"));
        c.arg("mcp")
            .arg("-c")
            .arg(&self.config)
            .args(args)
            .env("BURROW_ADMIN_SESSION_DIR", self.sessions())
            .env("BURROW_INSTANCE_DIR", self.dir.path().join("instances"))
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        c
    }

    pub fn assert_no_secret(&self, text: &str) {
        for s in &self.secrets {
            assert!(
                !text.contains(s.as_str()),
                "a secret reached the AI: {text}"
            );
        }
    }
}

pub fn visitor(w: &World, name: &str, secret: &Secret) -> Visitor {
    let dir = tempfile::tempdir().unwrap();
    let server = if common::ws_mode() {
        format!("ws://{}/burrow", w.addr)
    } else {
        w.addr.to_string()
    };
    let config = dir.path().join("visitor.yaml");
    burrow::base::fs::write_private(
        &config,
        &format!(
            "server: {server}\nname: {name}\nsecret: {}\n",
            secret.encode()
        ),
    )
    .unwrap();
    let secrets = [&w.home, &w.ops, &w.viewer, &w.plain]
        .iter()
        .map(|s| s.encode())
        .collect();
    Visitor {
        dir,
        config,
        secrets,
    }
}

pub struct Ai {
    pub client: RunningService<RoleClient, ()>,
    secrets: Vec<String>,
}

pub async fn ai(v: &Visitor, args: &[&str]) -> Ai {
    let child = TokioChildProcess::new(v.command(args)).unwrap();
    let client = ().serve(child).await.expect("burrow mcp answers over stdio");
    Ai {
        client,
        secrets: v.secrets.clone(),
    }
}

impl Ai {
    pub async fn tools(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .client
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        names
    }

    pub async fn call(&self, name: &str, args: Value) -> (bool, Value) {
        let mut p = CallToolRequestParams::new(name.to_string());
        if let Value::Object(o) = args {
            p = p.with_arguments(o);
        }
        let r: CallToolResult = match self.client.call_tool(p).await {
            Ok(r) => r,
            Err(e) => return (true, serde_json::json!({ "error": e.to_string() })),
        };
        let text = serde_json::to_string(&r).unwrap();
        for s in &self.secrets {
            assert!(
                !text.contains(s.as_str()),
                "a secret reached the AI: {text}"
            );
        }
        (
            r.is_error.unwrap_or(false),
            r.structured_content.unwrap_or(Value::Null),
        )
    }

    pub async fn ok(&self, name: &str, args: Value) -> Value {
        let (failed, v) = self.call(name, args).await;
        assert!(!failed, "{name} failed: {v}");
        v
    }

    pub async fn stop(self) {
        let _ = self.client.cancel().await;
    }
}
