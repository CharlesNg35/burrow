use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use burrow::wire::secret::Secret;
use serde_json::Value;

use super::releases::Releases;

pub const OLD: &str = "9.0.0-test.1";
pub const NEW: &str = "9.0.0-test.2";

pub fn build(marks: &[(&str, &str)]) -> Vec<u8> {
    let mut b = std::fs::read(env!("CARGO_BIN_EXE_burrow")).unwrap();
    b.push(b'\n');
    for (k, v) in marks {
        b.extend_from_slice(format!("BURROW-TEST-{k}={v}\n").as_bytes());
    }
    b
}

pub struct Hand {
    pub dir: tempfile::TempDir,
    pub binary: PathBuf,
    pub config: PathBuf,
    pub releases: Releases,
    keep: Arc<AtomicBool>,
    supervisor: Option<std::thread::JoinHandle<()>>,
}

impl Hand {
    pub fn new(server: &str, secret: &Secret) -> Hand {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join(if cfg!(windows) {
            "burrow.exe"
        } else {
            "burrow"
        });
        std::fs::write(&binary, build(&[("VERSION", OLD)])).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let config = dir.path().join("client.yaml");
        burrow::base::fs::write_private(
            &config,
            &format!(
                "server: {server}\nname: home\nsecret: {}\nallow: [127.0.0.1/32]\nupdate:\n  remote: true\n",
                secret.encode()
            ),
        )
        .unwrap();
        Hand {
            dir,
            binary,
            config,
            releases: Releases::start(),
            keep: Arc::new(AtomicBool::new(true)),
            supervisor: None,
        }
    }

    pub fn command(&self) -> Command {
        let mut c = Command::new(&self.binary);
        c.args(["client", "-c"])
            .arg(&self.config)
            .env("BURROW_UPDATE_URL", &self.releases.base)
            .env("BURROW_INSTANCE_DIR", self.dir.path().join("instances"))
            .env("BURROW_ADMIN_TEST_UPDATE_WINDOW", "4")
            .env_remove("HTTPS_PROXY")
            .env_remove("https_proxy")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        c
    }

    pub fn supervise(&mut self) {
        let mut command = self.command();
        let keep = self.keep.clone();
        self.supervisor = Some(std::thread::spawn(move || {
            while keep.load(Ordering::Acquire) {
                let Ok(mut child): std::io::Result<Child> = command.spawn() else {
                    return;
                };
                while keep.load(Ordering::Acquire) {
                    if child.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = child.kill();
                let _ = child.wait();
                std::thread::sleep(Duration::from_millis(300));
            }
        }));
    }

    pub fn record(&self) -> Value {
        let path = self.binary.with_extension("update.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_default()).unwrap_or_default()
    }
}

impl Drop for Hand {
    fn drop(&mut self) {
        self.keep.store(false, Ordering::Release);
        if let Some(s) = self.supervisor.take() {
            let _ = s.join();
        }
        if cfg!(windows) {
            let script = format!(
                "Get-Process burrow -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -eq '{}' }} | Stop-Process -Force",
                self.binary.display().to_string().replace('\'', "''")
            );
            let _ = Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .status();
        }
    }
}
