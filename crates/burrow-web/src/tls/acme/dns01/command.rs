use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use hickory_proto::rr::Name;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::{Fut, Provider, Refusal};

pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const SHOWN: usize = 300;
const KEEP: usize = 64 * 1024;

pub struct Command {
    path: PathBuf,
    timeout: Duration,
}

impl Command {
    pub fn new(path: PathBuf) -> Command {
        Command {
            path,
            timeout: COMMAND_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Command {
        self.timeout = timeout;
        self
    }

    async fn run(&self, action: &str, fqdn: &Name, value: &str) -> Result<(), Refusal> {
        let shown = self.path.display();
        let mut cmd = tokio::process::Command::new(&self.path);
        cmd.args([action, &fqdn.to_lowercase().to_ascii(), value])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if cfg!(windows)
            && let Some(root) = std::env::var_os("SYSTEMROOT")
        {
            cmd.env("SYSTEMROOT", root);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot run {shown}: {e}"))?;
        let (out, err) = (child.stdout.take(), child.stderr.take());
        let ran = {
            let all = async { tokio::join!(capped(out), capped(err), child.wait()) };
            tokio::time::timeout(self.timeout, all).await
        };
        let Ok((stdout, stderr, status)) = ran else {
            let _ = child.kill().await;
            return Err(Refusal::Failed(format!(
                "{shown} {action} did not finish within {} s and was stopped",
                self.timeout.as_secs()
            )));
        };
        let status = status.map_err(|e| format!("cannot run {shown}: {e}"))?;
        if status.success() {
            return Ok(());
        }
        let said = if stderr.0.iter().any(|b| !b.is_ascii_whitespace()) {
            stderr
        } else {
            stdout
        };
        let text = String::from_utf8_lossy(&said.0);
        let shown_text = match said.1 {
            true => format!("{} … (more output was cut)", head(&text)),
            false => tail(&text),
        };
        Err(Refusal::Failed(format!(
            "{shown} {action} failed ({status}): {shown_text}"
        )))
    }
}

async fn capped(pipe: Option<impl AsyncRead + Unpin>) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut cut = false;
    let Some(mut pipe) = pipe else {
        return (kept, cut);
    };
    let mut buf = [0u8; 8192];
    while let Ok(n) = pipe.read(&mut buf).await {
        if n == 0 {
            break;
        }
        let room = KEEP.saturating_sub(kept.len());
        cut |= n > room;
        kept.extend_from_slice(&buf[..n.min(room)]);
    }
    (kept, cut)
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn head(text: &str) -> String {
    one_line(text).chars().take(SHOWN).collect()
}

pub fn tail(text: &str) -> String {
    let one = one_line(text);
    let skip = one.chars().count().saturating_sub(SHOWN);
    one.chars().skip(skip).collect()
}

impl Provider for Command {
    fn present<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a> {
        Box::pin(self.run("present", fqdn, value))
    }

    fn cleanup<'a>(&'a self, fqdn: &'a Name, value: &'a str) -> Fut<'a> {
        Box::pin(self.run("cleanup", fqdn, value))
    }
}

#[cfg(all(test, unix))]
mod tests;
