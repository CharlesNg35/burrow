use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Setup {
    dir: tempfile::TempDir,
    locks: tempfile::TempDir,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let secret = burrow::secret::Secret::generate().encode();
    let cfg = format!(
        "listen: 127.0.0.1:{}\nclients:\n  home:\n    secret: {secret}\nservices:\n  web:\n    client: home\n    bind: 127.0.0.1:{}\n    target: 127.0.0.1:3000\n",
        free_port(),
        free_port()
    );
    let path = dir.path().join("server.yaml");
    std::fs::write(&path, cfg).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    Setup { dir, locks }
}

fn burrow(s: &Setup) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_burrow"));
    c.current_dir(s.dir.path())
        .env("BURROW_INSTANCE_DIR", s.locks.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    c
}

fn wait_running(child: &mut Child) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        assert!(child.try_wait().unwrap().is_none(), "server exited early");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn second_start_is_refused_and_crash_releases_the_lock() {
    let s = setup();
    let mut first = burrow(&s).arg("server").spawn().unwrap();
    wait_running(&mut first);

    let second = burrow(&s).arg("server").output().unwrap();
    assert_eq!(second.status.code(), Some(1));
    let err = String::from_utf8_lossy(&second.stderr);
    assert!(err.contains("is already running"), "{err}");
    assert!(err.contains(&format!("pid {}", first.id())), "{err}");

    first.kill().unwrap();
    first.wait().unwrap();

    let mut third = burrow(&s).arg("server").spawn().unwrap();
    wait_running(&mut third);
    third.kill().unwrap();
    third.wait().unwrap();
}

#[test]
fn config_is_found_in_the_current_folder() {
    let s = setup();
    let out = burrow(&s).arg("validate").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_file(s.dir.path().join("server.yaml")).unwrap();
    let out = burrow(&s).arg("server").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("burrow init server"));
}

#[test]
fn an_ad_hoc_visit_does_not_warn_that_the_file_has_no_forwards() {
    let s = setup();
    let secret = burrow::secret::Secret::generate().encode();
    std::fs::write(
        s.dir.path().join("visitor.yaml"),
        format!("server: 127.0.0.1:1\nname: laptop\nsecret: {secret}\n"),
    )
    .unwrap();
    let local = format!("127.0.0.1:{}", free_port());
    let mut child = burrow(&s)
        .args([
            "visit",
            "-c",
            "visitor.yaml",
            "--service",
            "web",
            "--local",
            &local,
        ])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    let _ = child.kill();
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("no forwards are defined"), "{err}");
    assert!(err.contains("service \"web\""), "{err}");
}
