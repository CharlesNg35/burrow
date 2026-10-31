mod common;
#[path = "reload/limits.rs"]
mod limits;
#[path = "reload/live.rs"]
mod live;
#[path = "reload/processes.rs"]
mod processes;
#[path = "reload/signals.rs"]
mod signals;
#[cfg(feature = "web")]
#[path = "reload/uploads.rs"]
mod uploads;

use std::net::SocketAddr;
use std::time::Duration;

use burrow::client::{Client, State};
use burrow::config::settings::ClientSettings;
use burrow::policy::allowlist::AllowList;
use burrow::wire::secret::Secret;
use burrow_config::Protocol;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use common::*;

async fn echoes(conn: &mut TcpStream) -> bool {
    tokio::time::timeout(Duration::from_secs(3), async {
        conn.write_all(b"ping").await.ok()?;
        let mut back = [0u8; 4];
        conn.read_exact(&mut back).await.ok()?;
        (&back == b"ping").then_some(())
    })
    .await
    .is_ok_and(|r| r.is_some())
}

async fn eventually(addr: SocketAddr) -> bool {
    within_tries(50, || async move {
        try_roundtrip(addr, b"hello").await.as_deref() == Some(b"hello")
    })
    .await
}

struct Live {
    server: RunningServer,
    server_settings: burrow::config::settings::ServerSettings,
    client: RunningClient,
    reload: mpsc::Sender<ClientSettings>,
    settings: ClientSettings,
    secret: Secret,
}

impl Live {
    async fn start(allow: &str) -> Live {
        Live::start_with(allow, |_| {}).await
    }

    async fn start_with(
        allow: &str,
        edit: impl FnOnce(&mut burrow::config::settings::ServerSettings),
    ) -> Live {
        Live::start_both(allow, edit, |_| {}).await
    }

    async fn start_both(
        allow: &str,
        edit: impl FnOnce(&mut burrow::config::settings::ServerSettings),
        edit_client: impl FnOnce(&mut ClientSettings),
    ) -> Live {
        logs();
        let echo = echo_server().await;
        let secret = Secret::generate();
        let mut server = server_settings(vec![secret.clone()]);
        server
            .services
            .insert("web".into(), service(echo, Protocol::Tcp));
        edit(&mut server);
        let server_settings = server.clone();
        let server = start_server(server).await;
        let mut settings = over_transport(client_settings(server.addr, secret.clone()));
        settings.allow = AllowList::Rules(vec![allow.parse().unwrap()]);
        edit_client(&mut settings);
        let (reload, reloads) = mpsc::channel(4);
        let mut client =
            start_client_with(Client::new(settings.clone()).with_reloads(reloads)).await;
        wait_state(&mut client.state, State::Connected).await;
        let shared = server.shared.clone();
        wait_for(|| shared.idle_channels("home") >= 1).await;
        Live {
            server,
            server_settings,
            client,
            reload,
            settings,
            secret,
        }
    }

    fn web(&self) -> SocketAddr {
        self.server.services["web"]
    }

    async fn change(&mut self, edit: impl FnOnce(&mut ClientSettings)) {
        edit(&mut self.settings);
        self.reload.send(self.settings.clone()).await.unwrap();
    }

    fn connected_secs(&self) -> u64 {
        self.server.shared.status().clients[0]
            .connected_secs
            .unwrap_or(0)
    }
}

mod process {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};

    pub struct Running {
        pub child: Child,
        pub log: PathBuf,
    }

    impl Running {
        pub fn start(role: &str, config: &Path, locks: &Path) -> Running {
            Running::spawn(
                Command::new(env!("CARGO_BIN_EXE_burrow")),
                role,
                config,
                locks,
            )
        }

        #[cfg(unix)]
        pub fn start_with_open_files(
            files: u32,
            role: &str,
            config: &Path,
            locks: &Path,
        ) -> Running {
            let mut sh = Command::new("sh");
            sh.arg("-c")
                .arg(format!("ulimit -n {files} && exec \"$0\" \"$@\""))
                .arg(env!("CARGO_BIN_EXE_burrow"));
            Running::spawn(sh, role, config, locks)
        }

        fn spawn(mut command: Command, role: &str, config: &Path, locks: &Path) -> Running {
            let log = config.with_extension("log");
            let child = command
                .arg(role)
                .arg("-c")
                .arg(config)
                .env("BURROW_INSTANCE_DIR", locks)
                .env("BURROW_LOG", "burrow=info")
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(&log).unwrap())
                .spawn()
                .unwrap();
            Running { child, log }
        }

        pub fn log(&self) -> String {
            std::fs::read_to_string(&self.log).unwrap_or_default()
        }

        pub async fn logged(&self, text: &str) -> bool {
            for _ in 0..100 {
                if self.log().contains(text) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            false
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    pub fn write(path: &Path, text: &str) {
        burrow::base::fs::write_private(path, text).unwrap();
    }

    pub fn server_line(addr: SocketAddr) -> String {
        if ws_mode() {
            format!("ws://{addr}/burrow")
        } else {
            addr.to_string()
        }
    }
}
