mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use burrow::wire::secret::Secret;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::TokioChildProcess;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use common::older::older;

const NEW: &str = env!("CARGO_BIN_EXE_burrow");

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Setup {
    dir: tempfile::TempDir,
    listen: u16,
    bind: u16,
    home: Secret,
    ops: Secret,
}

impl Setup {
    fn new() -> Setup {
        Setup {
            dir: tempfile::tempdir().unwrap(),
            listen: common::free_port(),
            bind: common::free_port(),
            home: Secret::generate(),
            ops: Secret::generate(),
        }
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let p = self.dir.path().join(name);
        burrow::base::fs::write_private(&p, text).unwrap();
        p
    }

    fn server(&self, echo: u16) -> PathBuf {
        self.write(
            "server.yaml",
            &format!(
                "listen: 127.0.0.1:{}\nclients:\n  home:\n    secret: {}\nvisitors:\n  ops:\n    secret: {}\n    admin: write\nservices:\n  web: {{ client: home, bind: 127.0.0.1:{}, target: 127.0.0.1:{echo} }}\n",
                self.listen,
                self.home.encode(),
                self.ops.encode(),
                self.bind
            ),
        )
    }

    fn client(&self, echo: u16) -> PathBuf {
        self.write(
            "client.yaml",
            &format!(
                "server: 127.0.0.1:{}\nname: home\nsecret: {}\nallow:\n  - 127.0.0.1/32:{echo}\n",
                self.listen,
                self.home.encode()
            ),
        )
    }

    fn visitor(&self) -> PathBuf {
        self.write(
            "visitor.yaml",
            &format!(
                "server: 127.0.0.1:{}\nname: ops\nsecret: {}\n",
                self.listen,
                self.ops.encode()
            ),
        )
    }

    fn command(&self, bin: &Path) -> Command {
        let mut c = Command::new(bin);
        c.env("BURROW_INSTANCE_DIR", self.dir.path().join("instances"))
            .env("BURROW_ADMIN_SESSION_DIR", self.dir.path().join("sessions"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        c
    }

    fn run(&self, bin: &Path, role: &str, config: &Path) -> Running {
        Running(
            self.command(bin)
                .arg(role)
                .arg("-c")
                .arg(config)
                .spawn()
                .unwrap(),
        )
    }
}

async fn tunnel_works(port: u16) -> bool {
    for _ in 0..100 {
        if let Ok(Ok(mut s)) = tokio::time::timeout(
            Duration::from_secs(1),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        {
            let mut got = [0u8; 5];
            let sent = s.write_all(b"hello").await.is_ok();
            let read = tokio::time::timeout(Duration::from_secs(2), s.read_exact(&mut got)).await;
            if sent && matches!(read, Ok(Ok(_))) && &got == b"hello" {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

async fn tunnel_between(server: &Path, client: &Path) {
    let s = Setup::new();
    let target = common::echo_server().await.port();
    let _server = s.run(server, "server", &s.server(target));
    let _client = s.run(client, "client", &s.client(target));
    assert!(
        tunnel_works(s.bind).await,
        "no tunnel between {} and {}",
        server.display(),
        client.display()
    );
}

#[tokio::test]
async fn a_new_client_tunnels_through_an_older_server() {
    let Some(old) = older() else { return };
    tunnel_between(&old, Path::new(NEW)).await;
}

#[tokio::test]
async fn an_older_client_tunnels_through_a_new_server() {
    let Some(old) = older() else { return };
    tunnel_between(Path::new(NEW), &old).await;
}

async fn call(
    c: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &str,
    args: Value,
) -> (bool, Value) {
    let mut p = CallToolRequestParams::new(name.to_string());
    if let Value::Object(o) = args {
        p = p.with_arguments(o);
    }
    let r = c.call_tool(p).await.unwrap();
    (
        r.is_error.unwrap_or(false),
        r.structured_content.unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn burrow_mcp_reads_and_changes_an_older_server() {
    let Some(old) = older() else { return };
    let s = Setup::new();
    let target = common::echo_server().await.port();
    let config = s.server(target);
    let _server = s.run(&old, "server", &config);
    let visitor = s.visitor();
    let mcp = || {
        let mut cmd = tokio::process::Command::from(s.command(Path::new(NEW)));
        cmd.arg("mcp")
            .arg("-c")
            .arg(&visitor)
            .arg("--allow-write")
            .kill_on_drop(true);
        TokioChildProcess::new(cmd).unwrap()
    };
    let mut client = None;
    for _ in 0..50 {
        match ().serve(mcp()).await {
            Ok(c) => {
                client = Some(c);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    let c = client.expect("burrow mcp connects to the older server");
    let tools: Vec<String> = c
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(tools.iter().any(|t| t == "apply_change"), "{tools:?}");
    let (failed, status) = call(&c, "get_status", json!({})).await;
    assert!(!failed && status["version"] == "0.1.2", "{status}");
    let (failed, cfg) = call(&c, "get_config", json!({})).await;
    assert!(!failed && cfg["rev"].is_string(), "{cfg}");
    let set = json!({ "ops": [{ "op": "set", "path": ["services", "web", "target"], "value": format!("127.0.0.1:{}", target + 1) }] });
    let (failed, plan) = call(&c, "check_change", set).await;
    assert!(!failed && plan["can_apply"] == true, "{plan}");
    let (failed, applied) = call(&c, "apply_change", json!({ "plan": plan["plan"] })).await;
    assert!(!failed, "{applied}");
    let written = std::fs::read_to_string(&config).unwrap();
    assert!(
        written.contains(&format!("target: 127.0.0.1:{}", target + 1)),
        "the older server wrote the change to its file: {written}"
    );
    let (failed, versions) = call(&c, "list_versions", json!({ "limit": 5 })).await;
    assert!(!failed, "{versions}");
    let rev = versions["entries"][0]["rev"].clone();
    let (failed, diffed) = call(&c, "list_versions", json!({ "limit": 1, "version": rev })).await;
    assert!(!failed && diffed["diff"]["unified"].is_string(), "{diffed}");
    let (failed, why) = call(&c, "list_versions", json!({ "entry": 1 })).await;
    assert!(failed && why["error"] == "no-entry-ids", "{why}");
    let _ = c.cancel().await;
}
