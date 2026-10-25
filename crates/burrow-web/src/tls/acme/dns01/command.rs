use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use hickory_proto::rr::Name;

use super::{Fut, Provider, Refusal};

pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const SHOWN: usize = 300;

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
        let out = tokio::time::timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| {
                format!(
                    "{shown} {action} did not finish within {} s and was stopped",
                    self.timeout.as_secs()
                )
            })?
            .map_err(|e| format!("cannot run {shown}: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let said = if out.stderr.iter().any(|b| !b.is_ascii_whitespace()) {
            out.stderr
        } else {
            out.stdout
        };
        Err(Refusal::Failed(format!(
            "{shown} {action} failed ({}): {}",
            out.status,
            tail(&String::from_utf8_lossy(&said))
        )))
    }
}

pub fn tail(text: &str) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
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
