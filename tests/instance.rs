mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::free_port;

struct Setup {
    dir: tempfile::TempDir,
    locks: tempfile::TempDir,
}

fn setup() -> Setup {
    setup_with("", "")
}

fn setup_with(top: &str, services: &str) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let secret = burrow::wire::secret::Secret::generate().encode();
    let cfg = format!(
        "listen: 127.0.0.1:{}\nclients:\n  home:\n    secret: {secret}\n{top}services:\n  web:\n    client: home\n    bind: 127.0.0.1:{}\n    target: 127.0.0.1:3000\n{services}",
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

    let mut c = burrow(&s);
    let log = logged(&s, c.arg("server"), "second");
    let (code, err) = finish(c.spawn().unwrap(), &log, Duration::from_secs(30));
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("is already running"), "{err}");
    assert!(err.contains(&format!("pid {}", first.id())), "{err}");

    first.kill().unwrap();
    first.wait().unwrap();

    let mut third = burrow(&s).arg("server").spawn().unwrap();
    wait_running(&mut third);
    third.kill().unwrap();
    third.wait().unwrap();
}

fn logged(s: &Setup, c: &mut Command, name: &str) -> PathBuf {
    let log = s.dir.path().join(format!("{name}.log"));
    c.stderr(std::fs::File::create(&log).unwrap());
    log
}

fn stopped(mut child: Child, log: &Path) -> String {
    let _ = child.kill();
    let _ = child.wait();
    std::fs::read_to_string(log).unwrap_or_default()
}

fn finish(mut child: Child, log: &Path, within: Duration) -> (Option<i32>, String) {
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() >= within {
            let text = stopped(child, log);
            panic!("burrow did not exit within {within:?}, so it was killed; its log:\n{text}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let code = child.wait().unwrap().code();
    (code, std::fs::read_to_string(log).unwrap_or_default())
}

#[cfg(feature = "web")]
fn until(child: &mut Child, within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while !done() {
        if start.elapsed() >= within || child.try_wait().unwrap().is_some() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

#[cfg(feature = "web")]
#[test]
fn a_second_server_that_fails_the_lock_leaves_the_uploads_alone() {
    let s = setup_with(
        &format!("web:\n  http: 127.0.0.1:{}\n", free_port()),
        "  site:\n    client: home\n    target: 127.0.0.1:8080\n    web: { hosts: [example.com], tls: off }\n",
    );
    let state = s.dir.path().join("state");
    let old = state.join("files").join("pair-00000000000000aa.pem");
    let server = |name: &str| {
        let mut c = burrow(&s);
        c.arg("server").arg("--state-dir").arg(&state);
        let log = logged(&s, &mut c, name);
        (c.spawn().unwrap(), log)
    };
    let (mut holder, holder_log) = server("holder");
    let id = holder.id();
    let locked = until(&mut holder, Duration::from_secs(30), || {
        burrow::instance::running_in(s.locks.path())
            .iter()
            .any(|r| r.pid == id)
    });
    if !locked {
        let text = stopped(holder, &holder_log);
        panic!("the first server did not take the lock; its log:\n{text}");
    }
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(&old, "x").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
        .unwrap();

    let (second, log) = server("second");
    let (code, err) = finish(second, &log, Duration::from_secs(30));
    stopped(holder, &holder_log);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("is already running"), "{err}");
    assert!(old.exists(), "the refused server removed an upload");

    let (mut third, log) = server("third");
    let pruned = until(&mut third, Duration::from_secs(30), || !old.exists());
    let err = stopped(third, &log);
    assert!(pruned, "the running server keeps an unused upload: {err}");
    assert!(err.contains("removed 1 uploaded files"), "{err}");
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
    let mut c = burrow(&s);
    let log = logged(&s, c.arg("server"), "missing");
    let (code, err) = finish(c.spawn().unwrap(), &log, Duration::from_secs(30));
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains(" init server"), "{err}");
}

#[test]
fn an_ad_hoc_visit_does_not_warn_that_the_file_has_no_forwards() {
    let s = setup();
    let secret = burrow::wire::secret::Secret::generate().encode();
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
    let stderr = child.stderr.take().unwrap();
    let err = common::read_until(stderr, Duration::from_secs(15), |e| {
        e.contains("service \"web\"")
    });
    let _ = child.kill();
    let _ = child.wait();
    assert!(!err.contains("no forwards are defined"), "{err}");
    assert!(err.contains("service \"web\""), "{err}");
}

#[test]
fn init_refuses_flags_that_are_not_for_the_role() {
    let s = setup();
    let secret = burrow::wire::secret::Secret::generate().encode();
    let cases: [(&[&str], &str); 4] = [
        (
            &["init", "server", "--name", "x", "-o", "a.yaml"],
            "--name is not for server configs",
        ),
        (
            &["init", "server", "--secret", &secret, "-o", "a.yaml"],
            "--secret is not for server configs",
        ),
        (
            &[
                "init",
                "client",
                "--server",
                "example.com",
                "--name",
                "home",
                "--secret",
                &secret,
                "--ws",
                "-o",
                "a.yaml",
            ],
            "--ws and its options are not for client configs",
        ),
        (
            &[
                "init",
                "visitor",
                "--server",
                "127.0.0.1:1",
                "--name",
                "l",
                "--secret",
                &secret,
                "--client-name",
                "x",
                "-o",
                "a.yaml",
            ],
            "the visitor's own name is --name",
        ),
    ];
    for (args, want) in cases {
        let out = burrow(&s).args(args).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {err}");
        assert!(err.contains(want), "{args:?}: {err}");
        assert!(
            !s.dir.path().join("a.yaml").exists(),
            "{args:?} wrote a file"
        );
    }
}

#[test]
fn validate_in_json_reports_an_unreadable_file_as_json() {
    let s = setup();
    std::fs::create_dir(s.dir.path().join("folder.yaml")).unwrap();
    let mut cases = vec![("nope.yaml", "does not exist")];
    if cfg!(unix) {
        cases.push(("folder.yaml", "is a directory, not a file"));
    }
    for (file, want) in cases {
        let out = burrow(&s)
            .args(["validate", "-c", file, "--format", "json"])
            .stdout(Stdio::piped())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
        assert_eq!(v["valid"], false);
        assert_eq!(v["errors"], 1);
        assert_eq!(v["diagnostics"][0]["code"], "read");
        let message = v["diagnostics"][0]["message"].as_str().unwrap();
        assert!(message.contains(want), "{message}");
    }
}

#[cfg(unix)]
#[test]
fn hints_quote_a_config_path_with_a_space() {
    use std::os::unix::fs::PermissionsExt;
    let s = setup();
    let dir = s.dir.path().join("my dir");
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("server.yaml");
    std::fs::copy(s.dir.path().join("server.yaml"), &path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = burrow(&s)
        .args(["validate", "-c"])
        .arg(&path)
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains(&format!("chmod 600 '{}'", path.display())),
        "{text}"
    );
}

#[test]
fn the_server_says_which_service_targets_a_client_does_not_allow() {
    let s = setup();
    let secret = burrow::wire::secret::Secret::generate().encode();
    let port = free_port();
    std::fs::write(
        s.dir.path().join("server.yaml"),
        format!(
            "listen: 127.0.0.1:{port}\nclients:\n  home:\n    secret: {secret}\nservices:\n  web:\n    client: home\n    bind: 127.0.0.1:{}\n    target: 127.0.0.1:3000\n  ssh:\n    client: home\n    bind: 127.0.0.1:{}\n    target: 127.0.0.1:22\n",
            free_port(),
            free_port()
        ),
    )
    .unwrap();
    std::fs::write(
        s.dir.path().join("client.yaml"),
        format!(
            "server: 127.0.0.1:{port}\nname: home\nsecret: {secret}\nallow: [127.0.0.1/32:22]\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    for f in ["server.yaml", "client.yaml"] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(s.dir.path().join(f), std::fs::Permissions::from_mode(0o600))
            .unwrap();
    }
    let mut server = burrow(&s)
        .args(["server", "-c", "server.yaml"])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let mut client = burrow(&s)
        .args(["client", "-c", "client.yaml"])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(2500));
    assert!(server.try_wait().unwrap().is_none(), "the server stopped");
    let _ = client.kill();
    let _ = client.wait();
    let _ = server.kill();
    let out = server.wait_with_output().unwrap();
    let log = String::from_utf8_lossy(&out.stderr);
    assert!(
        log.contains(
            "client \"home\" does not allow the target of service \"web\" (127.0.0.1:3000)"
        ),
        "{log}"
    );
    let warned = log.lines().find(|l| l.contains("does not allow")).unwrap();
    assert!(!warned.contains("\"ssh\""), "{warned}");
}

type StartCase<'a> = (&'a str, &'a str, &'a str, &'a [(&'a str, &'a str)], i32);

fn start_code(top: &str, services: &str, files: &[(&str, &str)]) -> (Option<i32>, String) {
    let s = setup_with(top, services);
    for (name, body) in files {
        let path = s.dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let state = s.dir.path().join("state");
    let out = burrow(&s)
        .args(["server", "-c", "server.yaml", "--state-dir"])
        .arg(&state)
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_start_that_fails_only_on_unreadable_web_files_exits_1_for_a_retry_and_bad_contents_exit_3() {
    let https = format!("web:\n  https: 127.0.0.1:{}\n", free_port());
    let pair = "{ cert: live/fullchain.pem, key: live/privkey.pem }";
    let site = |web: &str| format!("  nas:\n    client: home\n    target: 8000\n    web: {web}\n");
    let own = site(&format!("{{ hosts: [nas.example.com], tls: {pair} }}"));
    let shared = format!(
        "{https}  certificates:\n    - {{ hosts: [\"*.example.com\"], {} }}\n",
        &pair[2..pair.len() - 2]
    );
    let cas = site(
        "{ hosts: [nas.lan], tls: internal, client_auth: { ca: certs/devices.pem }, target_tls: { ca: certs/app-ca.pem } }",
    );
    let acme = "web:\n  https: 443\n  http: 80\n  letsencrypt: { email: ops@example.com, ca: certs/acme-ca.pem }\n";
    let junk = [
        ("live/fullchain.pem", "not a certificate"),
        ("live/privkey.pem", "not a key"),
    ];
    let cases: [StartCase; 6] = [
        ("site files", &https, &own, &[], 1),
        (
            "web.certificates",
            &shared,
            &site("nas.example.com"),
            &[],
            1,
        ),
        ("device and app CAs", &https, &cas, &[], 1),
        ("letsencrypt.ca", acme, &site("nas.example.com"), &[], 1),
        ("site files that are not PEM", &https, &own, &junk, 3),
        (
            "one unreadable, one not a CA",
            &https,
            &cas,
            &[("certs/app-ca.pem", "x")],
            3,
        ),
    ];
    for (what, top, services, files, want) in cases {
        let (code, err) = start_code(top, services, files);
        assert_eq!(code, Some(want), "{what}: {err}");
        assert!(err.contains("not starting"), "{what}: {err}");
    }
}

#[cfg(target_os = "linux")]
fn udp_buffer_is_capped() -> bool {
    let want = 7 << 20;
    let probe = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None).unwrap();
    if rustix::net::sockopt::set_socket_recv_buffer_size_force(&probe, want).is_err() {
        let _ = probe.set_recv_buffer_size(want);
    }
    probe.recv_buffer_size().unwrap() < want / 2
}

#[cfg(target_os = "linux")]
#[test]
fn a_capped_udp_receive_buffer_is_logged_once_for_all_ports() {
    let udp = format!(
        "  dns:\n    client: home\n    protocol: udp\n    bind: 127.0.0.1:{}\n    target: 127.0.0.1:53\n  game:\n    client: home\n    protocol: udp\n    bind: 127.0.0.1:{}\n    target: 127.0.0.1:27015\n",
        free_port(),
        free_port()
    );
    let s = setup_with("", &udp);
    let mut c = burrow(&s);
    let log = logged(&s, c.arg("server"), "server");
    let mut child = c.spawn().unwrap();
    wait_running(&mut child);
    let text = stopped(child, &log);
    let warned = text.lines().filter(|l| l.contains("well below")).count();
    let expected = usize::from(udp_buffer_is_capped());
    assert_eq!(warned, expected, "{text}");
    if expected == 1 {
        assert!(text.contains("net.core.rmem_max"), "{text}");
        assert!(text.contains("CAP_NET_ADMIN"), "{text}");
    }
}
