#[cfg(unix)]
use std::net::SocketAddr;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use burrow::wire::secret::Secret;
#[cfg(unix)]
use tokio::net::TcpStream;

#[cfg(unix)]
use super::live::free_local;
#[cfg(unix)]
use super::process;

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_whose_stderr_went_away_keeps_running_and_reloading() {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let (listen, web) = (free_local(), free_local());
    let config = dir.path().join("server.yaml");
    let yaml = |services: &str| {
        format!(
            "listen: {listen}\nclients:\n  home: {{ secret: {} }}\nservices:{services}\n",
            Secret::generate().encode()
        )
    };
    process::write(&config, &yaml(" {}"));
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut child = Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["server", "-c"])
        .arg(&config)
        .env("BURROW_INSTANCE_DIR", locks.path())
        .env("BURROW_LOG", "burrow=info")
        .stdout(Stdio::null())
        .stderr(writer)
        .spawn()
        .unwrap();
    assert!(eventually_listening(listen).await);
    let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::HUP).unwrap();
    process::write(
        &config,
        &yaml(&format!(
            "\n  web: {{ client: home, bind: {web}, target: 127.0.0.1:9 }}"
        )),
    );
    let listening = eventually_listening(web).await;
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    assert!(alive, "the server died");
    assert!(listening, "the reload was not applied");
}

#[cfg(unix)]
async fn eventually_listening(addr: SocketAddr) -> bool {
    super::common::within_tries(
        100,
        || async move { TcpStream::connect(addr).await.is_ok() },
    )
    .await
}

#[cfg(unix)]
#[test]
fn a_failing_command_whose_stderr_went_away_exits_with_its_code() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["server", "-c", "/nonexistent/burrow/server.yaml"])
        .stdout(std::process::Stdio::null())
        .stderr(writer)
        .status()
        .unwrap();
    let code = status.code();
    assert!(code.is_some_and(|c| c != 0 && c != 101), "{status:?}");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sighup_as_soon_as_the_port_is_open_does_not_kill_the_server() {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let listen = free_local();
    let config = dir.path().join("server.yaml");
    process::write(
        &config,
        &format!(
            "listen: {listen}\nclients:\n  home: {{ secret: {} }}\nservices: {{}}\n",
            Secret::generate().encode()
        ),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["server", "-c"])
        .arg(&config)
        .env("BURROW_INSTANCE_DIR", locks.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
    let up = eventually_listening(listen).await;
    for _ in 0..20 {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::HUP);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        up && alive,
        "a SIGHUP once the port is open killed the server"
    );
}
