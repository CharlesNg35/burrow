use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use burrow::metrics::Status;
use burrow::secret::Secret;

const BIN: &str = env!("CARGO_BIN_EXE_burrow");

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn server_config(dir: &Path, file: &str) -> PathBuf {
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

fn start(instances: &Path, config: &Path) -> Running {
    for _ in 0..5 {
        let child = Command::new(BIN)
            .args(["server", "-c"])
            .arg(config)
            .env("BURROW_INSTANCE_DIR", instances)
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

fn status(instances: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .arg("status")
        .args(args)
        .env("BURROW_INSTANCE_DIR", instances)
        .env("NO_COLOR", "1")
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn status_works_without_a_metrics_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    let config = server_config(dir.path(), "server.yaml");
    let _server = start(&instances, &config);
    let o = status(&instances, &[]);
    assert!(o.status.success(), "{}", text(&o));
    let out = text(&o);
    assert!(
        out.starts_with(&format!("burrow {}  ·  up ", env!("CARGO_PKG_VERSION"))),
        "{out}"
    );
    assert!(out.contains("CLIENTS  0/1 connected"), "{out}");
    assert!(out.contains("home  offline  never seen"), "{out}");
    assert!(out.contains("web: limit 5.0 MB/s"), "{out}");
    let o = status(&instances, &["-c", config.to_str().unwrap(), "--json"]);
    let list: Vec<serde_json::Value> = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(list.len(), 1);
    let s: Status = serde_json::from_value(list[0]["status"].clone()).unwrap();
    assert_eq!(s.services[0].name, "web");
    assert_eq!(s.services[0].bandwidth, Some(5_000_000));
}

#[test]
fn several_servers_are_shown_and_picked_by_config() {
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    let a = server_config(dir.path(), "a.yaml");
    let b = server_config(dir.path(), "b.yaml");
    let _a = start(&instances, &a);
    let _b = start(&instances, &b);
    let out = text(&status(&instances, &[]));
    for config in [&a, &b] {
        let shown = std::fs::canonicalize(config).unwrap().display().to_string();
        let shown = shown.strip_prefix(r"\\?\").unwrap_or(&shown);
        assert!(out.contains(&format!("== {shown}")), "{out}");
    }
    let only_b = text(&status(&instances, &["-c", b.to_str().unwrap()]));
    assert!(
        !only_b.contains("=="),
        "one server needs no heading: {only_b}"
    );
    let o = status(
        &instances,
        &["-c", dir.path().join("c.yaml").to_str().unwrap()],
    );
    assert!(!o.status.success());
    assert!(
        text(&o).contains("no running burrow server uses"),
        "{}",
        text(&o)
    );
}

#[test]
fn a_stopped_server_leaves_nothing_to_show() {
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    let config = server_config(dir.path(), "server.yaml");
    let mut server = start(&instances, &config);
    server.0.kill().unwrap();
    server.0.wait().unwrap();
    let o = status(&instances, &[]);
    assert!(!o.status.success());
    assert!(
        text(&o).contains("no burrow server is running"),
        "{}",
        text(&o)
    );
    let left: Vec<_> = std::fs::read_dir(&instances)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "status"))
        .collect();
    assert!(left.is_empty(), "the stale status file is removed");
}

#[cfg(unix)]
#[test]
fn files_of_other_users_are_counted_and_point_to_sudo() {
    use std::os::unix::fs::PermissionsExt;
    if std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    let config = server_config(dir.path(), "server.yaml");
    let server = start(&instances, &config);
    let file = instances.join(format!("{}.status", server.0.id()));
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    let o = status(&instances, &[]);
    assert!(!o.status.success());
    assert!(text(&o).contains("belong to another user"), "{}", text(&o));
    assert!(text(&o).contains("sudo"), "{}", text(&o));
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn the_status_endpoint_needs_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    let config = server_config(dir.path(), "server.yaml");
    let server = start(&instances, &config);
    let file = instances.join(format!("{}.status", server.0.id()));
    let entry: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "only the owner may read the token");
    }
    let port = entry["port"].as_u64().unwrap() as u16;
    let ask = |auth: &str| {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(format!("GET /status HTTP/1.1\r\n{auth}\r\n").as_bytes())
            .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    };
    assert!(ask("").starts_with("HTTP/1.1 401"));
    assert!(ask("Authorization: Bearer nope\r\n").starts_with("HTTP/1.1 401"));
    let token = entry["token"].as_str().unwrap();
    assert!(ask(&format!("Authorization: Bearer {token}\r\n")).starts_with("HTTP/1.1 200"));
}

#[test]
fn follow_keeps_refreshing_with_rates() {
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    let config = server_config(dir.path(), "server.yaml");
    let _server = start(&instances, &config);
    let mut child = Command::new(BIN)
        .args(["status", "-f", "--interval", "0.2"])
        .env("BURROW_INSTANCE_DIR", &instances)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    child.kill().unwrap();
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let _ = child.wait();
    let frames = out.matches("CLIENTS").count();
    assert!(frames >= 3, "{frames} frames:\n{out}");
    assert!(out.contains("CONN/S"), "{out}");
    let o = Command::new(BIN)
        .args(["status", "--interval", "2"])
        .output()
        .unwrap();
    assert!(!o.status.success(), "--interval needs --follow");
}

#[cfg(unix)]
fn sleeper() -> Running {
    Running(Command::new("sleep").arg("60").spawn().unwrap())
}

#[cfg(unix)]
fn plant(instances: &Path, pid: u32, port: u16, token: &str) {
    std::fs::create_dir_all(instances).unwrap();
    let entry = serde_json::json!({"config": "/etc/burrow/server.yaml", "pid": pid, "port": port, "token": token});
    std::fs::write(instances.join(format!("{pid}.status")), entry.to_string()).unwrap();
}

#[cfg(unix)]
#[test]
fn planted_status_files_cannot_hang_inject_or_paint_the_terminal() {
    use std::net::TcpListener;
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    std::fs::create_dir_all(&instances).unwrap();
    let token = "ab".repeat(24);

    let fifo = sleeper();
    let made = Command::new("mkfifo")
        .arg(instances.join(format!("{}.status", fifo.0.id())))
        .status()
        .unwrap();
    assert!(made.success());

    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let silent_port = silent.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for c in silent.incoming().flatten() {
            held.push(c);
        }
    });
    let slow = sleeper();
    plant(&instances, slow.0.id(), silent_port, &token);

    let trap = TcpListener::bind("127.0.0.1:0").unwrap();
    let trap_port = trap.local_addr().unwrap().port();
    trap.set_nonblocking(true).unwrap();
    let injected = sleeper();
    plant(
        &instances,
        injected.0.id(),
        trap_port,
        &format!("{token}\r\nX-Injected: yes"),
    );

    #[cfg(target_os = "linux")]
    plant(&instances, 1, trap_port, &token);

    let painter = TcpListener::bind("127.0.0.1:0").unwrap();
    let painter_port = painter.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut c in painter.incoming().flatten() {
            let mut head = [0u8; 4096];
            let _ = c.read(&mut head);
            let body = serde_json::json!({
                "version": "6.6.6\u{1b}]0;PWNED\u{7}\u{1b}[2J",
                "uptime_secs": 1,
                "listening": ["clients on \u{1b}[31mred"],
                "handshakes_refused": 0,
                "udp_peers": 0,
                "clients": [],
                "services": [],
                "visitors": []
            })
            .to_string();
            let _ = write!(
                c,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let shown = sleeper();
    plant(&instances, shown.0.id(), painter_port, &token);

    let began = std::time::Instant::now();
    let o = status(&instances, &[]);
    let took = began.elapsed();
    assert!(took < Duration::from_secs(4), "burrow status took {took:?}");
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.contains("burrow 6.6.6]0;PWNED"), "{out}");
    assert!(!out.contains('\u{1b}') && !out.contains('\u{7}'), "{out:?}");
    assert!(
        trap.accept().is_err(),
        "nothing may connect to the port of a bad entry"
    );
    drop((fifo, slow, injected, shown));
}

#[cfg(unix)]
#[test]
fn a_paused_server_keeps_its_status_file_and_shows_again() {
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    let config = server_config(dir.path(), "server.yaml");
    let server = start(&instances, &config);
    let pid = server.0.id().to_string();
    let file = instances.join(format!("{pid}.status"));
    let signal = |sig: &str| {
        assert!(
            Command::new("kill")
                .args([sig, &pid])
                .status()
                .unwrap()
                .success()
        );
    };
    signal("-STOP");
    for _ in 0..100 {
        let state = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&state.stdout)
            .trim_start()
            .starts_with('T')
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    let began = std::time::Instant::now();
    let o = status(&instances, &[]);
    let took = began.elapsed();
    signal("-CONT");
    assert!(took < Duration::from_secs(4), "burrow status took {took:?}");
    assert!(!o.status.success(), "{}", text(&o));
    assert!(file.exists(), "the status file of a paused server is kept");
    let o = status(&instances, &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("CLIENTS"), "{}", text(&o));
}

#[cfg(target_os = "linux")]
#[test]
fn a_server_raises_its_open_file_limit_to_the_hard_limit() {
    let dir = tempfile::tempdir().unwrap();
    let instances = dir.path().join("instances");
    let config = server_config(dir.path(), "server.yaml");
    let child = Command::new("sh")
        .args(["-c", "ulimit -Sn 256 && exec \"$0\" server -c \"$1\""])
        .arg(BIN)
        .arg(&config)
        .env("BURROW_INSTANCE_DIR", &instances)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let server = Running(child);
    let pid = server.0.id();
    let mut soft_hard = None;
    for _ in 0..100 {
        if instances.join(format!("{pid}.status")).exists() {
            let limits = std::fs::read_to_string(format!("/proc/{pid}/limits")).unwrap();
            let line = limits
                .lines()
                .find(|l| l.starts_with("Max open files"))
                .unwrap()
                .to_string();
            let fields: Vec<&str> = line.split_whitespace().collect();
            soft_hard = Some((fields[3].to_string(), fields[4].to_string()));
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let (soft, hard) = soft_hard.expect("the server did not start");
    assert_ne!(soft, "256", "the soft limit was not raised");
    assert_eq!(soft, hard);
}
