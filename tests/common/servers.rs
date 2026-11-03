use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use burrow::wire::secret::Secret;

use super::free_port;

const BIN: &str = env!("CARGO_BIN_EXE_burrow");

pub struct Running(pub Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn server_config(dir: &Path, file: &str) -> PathBuf {
    let config = dir.join(file);
    std::fs::write(
        &config,
        format!(
            "listen: 127.0.0.1:{}\nclients:\n  home:\n    secret: {}\nservices:\n  web:\n    client: home\n    bind: 127.0.0.1:{}\n    target: 3000\n    bandwidth: 40mbit\n",
            free_port(),
            Secret::generate().encode(),
            free_port()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    config
}

pub fn start(instances: &Path, config: &Path) -> Running {
    start_with(instances, config, &[])
}

pub fn start_with(instances: &Path, config: &Path, env: &[(&str, &str)]) -> Running {
    for _ in 0..5 {
        let child = Command::new(BIN)
            .args(["server", "-c"])
            .arg(config)
            .env("BURROW_INSTANCE_DIR", instances)
            .envs(env.iter().copied())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut running = Running(child);
        for _ in 0..100 {
            let ready = std::fs::read_dir(instances).is_ok_and(|d| {
                d.flatten().any(|e| {
                    e.file_name().to_string_lossy() == format!("{}.status", running.0.id())
                })
            });
            if ready {
                return running;
            }
            if running.0.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let name = config.file_name().unwrap().to_string_lossy().into_owned();
        server_config(config.parent().unwrap(), &name);
    }
    panic!("the server did not publish its status");
}
