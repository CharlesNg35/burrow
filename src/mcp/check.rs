use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use rmcp::ServiceExt;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

use super::mode::{Mode, Writes};
use super::tools::{Notices, Tools};
use super::upstream::Upstream;
use super::{CLIENT, setup};
use crate::admin_link::client::Link;
use crate::base::failure::Failure;
use crate::config::settings::VisitorSettings;

struct App {
    tx: WriteHalf<DuplexStream>,
    rx: Lines<BufReader<ReadHalf<DuplexStream>>>,
    next: u64,
}

fn broke(what: &str) -> Failure {
    Failure::new(format!(
        "burrow mcp did not answer {what} the way an AI app expects"
    ))
}

impl App {
    async fn send(&mut self, v: &Value) -> Result<(), Failure> {
        let mut line = serde_json::to_vec(v).map_err(|e| Failure::new(e.to_string()))?;
        line.push(b'\n');
        self.tx
            .write_all(&line)
            .await
            .map_err(|e| Failure::new(e.to_string()))
    }

    async fn ask(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        let id = self.next;
        self.next += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;
        while let Ok(Some(line)) = self.rx.next_line().await {
            let v: Value = serde_json::from_str(&line).map_err(|_| broke(method))?;
            if v["id"] == id {
                return v.get("result").cloned().ok_or_else(|| broke(method));
            }
        }
        Err(broke(method))
    }
}

fn level_word(mode: Mode) -> &'static str {
    match mode {
        Mode::ReadOnly => "read",
        Mode::CheckOnly | Mode::Write => "write",
    }
}

fn writes_line(mode: Mode) -> String {
    match mode {
        Mode::Write => "allowed".into(),
        Mode::CheckOnly => format!(
            "off; changes can be checked, not applied; add {} to apply them",
            setup::ALLOW_WRITE
        ),
        Mode::ReadOnly => {
            "off; the visitor has admin: read, so changes cannot be checked or applied".into()
        }
    }
}

pub async fn run(
    settings: VisitorSettings,
    config: &Path,
    writes: Writes,
    out: &mut dyn Write,
) -> Result<u8, Failure> {
    let say = |out: &mut dyn Write, line: String| crate::base::output::say(out, line);
    let (visitor, server) = (settings.name.clone(), settings.server.clone());
    say(
        out,
        format!(
            "  config:  {} (visitor \"{visitor}\")",
            setup::absolute(config).display()
        ),
    )?;
    let link = Arc::new(Link::new(settings, CLIENT));
    let up = Upstream::start(link.clone(), writes, None);
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let running = tokio::spawn(Tools::new(up.clone(), Notices::Never).serve(theirs));
    let (rx, tx) = tokio::io::split(ours);
    let mut app = App {
        tx,
        rx: BufReader::new(rx).lines(),
        next: 1,
    };
    let checked = async {
        let hello = json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "burrow-mcp-check", "version": env!("CARGO_PKG_VERSION") }
        });
        app.ask("initialize", hello).await?;
        app.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await?;
        let status = app
            .ask(
                "tools/call",
                json!({ "name": "get_status", "arguments": {} }),
            )
            .await?;
        let content = &status["structuredContent"];
        if status["isError"].as_bool().unwrap_or(false) {
            let hint = content["hint"].as_str().unwrap_or_default();
            let f = Failure::new(content["message"].as_str().unwrap_or("get_status failed"));
            return Err(if hint.is_empty() { f } else { f.hint(hint) });
        }
        let tools = app.ask("tools/list", json!({})).await?;
        Ok(tools["tools"]
            .as_array()
            .map(|t| {
                t.iter()
                    .filter_map(|t| t["name"].as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default())
    };
    let tools = checked.await;
    running.abort();
    let _ = tokio::time::timeout(std::time::Duration::from_millis(300), link.close()).await;
    let mut tools = tools?;
    tools.sort();
    let mode = up.mode();
    say(out, format!("  server:  reached {server}"))?;
    say(out, format!("  admin:   {}", level_word(mode)))?;
    say(out, format!("  writes:  {}", writes_line(mode)))?;
    say(
        out,
        format!("  tools:   {} ({})", tools.len(), tools.join(", ")),
    )?;
    say(
        out,
        format!(
            "ok: an AI app that starts burrow mcp with this config gets these tools ({} mode)",
            mode.word()
        ),
    )?;
    Ok(0)
}
