use std::time::Duration;

use super::common;
use super::world;

#[tokio::test]
async fn burrow_admin_stops_at_once_when_it_cannot_write_the_launch_page() {
    let w = world().await;
    let dir = tempfile::tempdir().unwrap();
    let server = if common::ws_mode() {
        format!("ws://{}/burrow", w.addr)
    } else {
        w.addr.to_string()
    };
    let config = dir.path().join("visitor.yaml");
    burrow::base::fs::write_private(
        &config,
        &format!("server: {server}\nname: ops\nsecret: {}\n", w.ops.encode()),
    )
    .unwrap();
    let blocker = dir.path().join("a-file");
    std::fs::write(&blocker, "x").unwrap();
    let sessions = dir.path().join("sessions");
    let started = std::time::Instant::now();
    let run = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_burrow"))
            .arg("admin")
            .arg("-c")
            .arg(&config)
            .args(["--port", "0", "--idle-exit", "60s"])
            .env("XDG_RUNTIME_DIR", blocker.join("x"))
            .env("TMPDIR", &blocker)
            .env("TMP", &blocker)
            .env("TEMP", &blocker)
            .env("BURROW_ADMIN_SESSION_DIR", &sessions)
            .output()
    });
    let out = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("burrow admin stopped long before its idle exit")
        .unwrap()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("private launch file"), "{stderr}");
    assert!(started.elapsed() < Duration::from_secs(30));
    let _ = w.stop.send(true);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_launch_page_goes_to_the_users_own_runtime_folder() {
    use std::os::unix::fs::PermissionsExt;
    let w = world().await;
    let dir = tempfile::tempdir().unwrap();
    let server = if common::ws_mode() {
        format!("ws://{}/burrow", w.addr)
    } else {
        w.addr.to_string()
    };
    let config = dir.path().join("visitor.yaml");
    burrow::base::fs::write_private(
        &config,
        &format!("server: {server}\nname: ops\nsecret: {}\n", w.ops.encode()),
    )
    .unwrap();
    let run = dir.path().join("run");
    std::fs::create_dir(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let opened = dir.path().join("opened.txt");
    let opener = bin.join("xdg-open");
    std::fs::write(
        &opener,
        format!("#!/bin/sh\necho \"$1\" > {}\n", opened.display()),
    )
    .unwrap();
    std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_burrow"))
        .arg("admin")
        .arg("-c")
        .arg(&config)
        .args(["--port", "0", "--idle-exit", "60s"])
        .env("XDG_RUNTIME_DIR", &run)
        .env("PATH", path)
        .env("BURROW_ADMIN_SESSION_DIR", dir.path().join("sessions"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let page = loop {
        if let Ok(text) = std::fs::read_to_string(&opened)
            && !text.trim().is_empty()
        {
            break std::path::PathBuf::from(text.trim());
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the browser was not opened"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let folder = run.join("burrow");
    assert_eq!(page.parent(), Some(folder.as_path()), "{}", page.display());
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode(&folder), 0o700);
    assert_eq!(mode(&page), 0o600);
    let _ = child.kill();
    let _ = child.wait();
    let _ = w.stop.send(true);
}

#[tokio::test]
async fn burrow_admin_stops_at_once_when_it_cannot_write_the_url_file() {
    let w = world().await;
    let dir = tempfile::tempdir().unwrap();
    let server = if common::ws_mode() {
        format!("ws://{}/burrow", w.addr)
    } else {
        w.addr.to_string()
    };
    let config = dir.path().join("visitor.yaml");
    burrow::base::fs::write_private(
        &config,
        &format!("server: {server}\nname: ops\nsecret: {}\n", w.ops.encode()),
    )
    .unwrap();
    let missing = dir.path().join("no-such-dir").join("url.txt");
    let sessions = dir.path().join("admin-sessions");
    let started = std::time::Instant::now();
    let run = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_burrow"))
            .arg("admin")
            .arg("-c")
            .arg(&config)
            .args([
                "--no-open",
                "--print-url",
                "--port",
                "0",
                "--idle-exit",
                "60s",
                "--url-file",
            ])
            .arg(&missing)
            .env("BURROW_ADMIN_SESSION_DIR", sessions)
            .output()
    });
    let out = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .expect("burrow admin stopped long before its idle exit")
        .unwrap()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("cannot write"), "{stderr}");
    assert!(!stderr.contains("no browser was connected"), "{stderr}");
    assert!(started.elapsed() < Duration::from_secs(30));
    let _ = w.stop.send(true);
}

struct AdminRun {
    child: std::process::Child,
    lines: std::sync::mpsc::Receiver<String>,
}

impl AdminRun {
    fn start(config: &std::path::Path, port: Option<&str>) -> AdminRun {
        use std::io::BufRead;
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_burrow"));
        cmd.arg("admin").arg("-c").arg(config).args([
            "--no-open",
            "--print-url",
            "--idle-exit",
            "30s",
        ]);
        let sessions = config.parent().unwrap_or(config).join("admin-sessions");
        cmd.env("BURROW_ADMIN_SESSION_DIR", sessions);
        if let Some(p) = port {
            cmd.args(["--port", p]);
        }
        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, lines) = std::sync::mpsc::channel();
        for out in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for l in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    let _ = tx.send(l);
                }
            });
        }
        AdminRun { child, lines }
    }

    fn until(&self, what: impl Fn(&str) -> bool) -> Vec<String> {
        let mut seen = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match self.lines.recv_timeout(left) {
                Ok(l) => {
                    let done = what(&l);
                    seen.push(l);
                    if done {
                        return seen;
                    }
                }
                Err(_) => break,
            }
        }
        panic!("burrow admin did not print what was expected: {seen:?}")
    }
}

impl Drop for AdminRun {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn burrow_admin_listens_on_7839_unless_it_is_taken_or_a_port_is_given() {
    if std::net::TcpListener::bind("127.0.0.1:7839").is_err() {
        println!("skipped: 127.0.0.1:7839 is in use on this machine");
        return;
    }
    let w = world().await;
    let dir = tempfile::tempdir().unwrap();
    let server = if common::ws_mode() {
        format!("ws://{}/burrow", w.addr)
    } else {
        w.addr.to_string()
    };
    let config = dir.path().join("visitor.yaml");
    burrow::base::fs::write_private(
        &config,
        &format!("server: {server}\nname: ops\nsecret: {}\n", w.ops.encode()),
    )
    .unwrap();
    let config2 = config.clone();
    let seen = tokio::task::spawn_blocking(move || {
        let first = AdminRun::start(&config2, None);
        let seen = first.until(|l| l.contains("token="));
        drop(first);
        seen
    })
    .await
    .unwrap();
    assert!(
        seen.iter().any(|l| l.contains("http://127.0.0.1:7839/")),
        "{seen:?}"
    );
    let config2 = config.clone();
    tokio::task::spawn_blocking(move || {
        let held = std::net::TcpListener::bind("127.0.0.1:7839").unwrap();
        let fallback = AdminRun::start(&config2, None);
        let note = |l: &str| {
            l.contains("port 7839 is in use")
                || l.contains("port 7839 of your last session is taken")
        };
        let mut seen = fallback.until(|l| l.contains("token="));
        if !seen.iter().any(|l| note(l)) {
            seen.extend(fallback.until(note));
        }
        assert!(seen.iter().any(|l| note(l)), "{seen:?}");
        assert!(
            !seen.iter().any(|l| l.contains("127.0.0.1:7839/")),
            "{seen:?}"
        );
        drop(fallback);
        let mut strict = AdminRun::start(&config2, Some("7839"));
        let seen = strict.until(|l| l.contains("cannot listen on 127.0.0.1:7839"));
        assert!(!seen.iter().any(|l| l.contains("token=")), "{seen:?}");
        let status = strict.child.wait().unwrap();
        assert_eq!(status.code(), Some(1));
        let random = AdminRun::start(&config2, Some("0"));
        let seen = random.until(|l| l.contains("token="));
        assert!(!seen.iter().any(|l| l.contains(":7839/")), "{seen:?}");
        drop(held);
    })
    .await
    .unwrap();
    let _ = w.stop.send(true);
}
