mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Setup {
    dir: tempfile::TempDir,
    locks: tempfile::TempDir,
    tmp: tempfile::TempDir,
}

fn setup() -> Setup {
    Setup {
        dir: tempfile::tempdir().unwrap(),
        locks: tempfile::tempdir().unwrap(),
        tmp: tempfile::tempdir().unwrap(),
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let left: Vec<PathBuf> = std::fs::read_dir(self.tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert!(left.is_empty(), "private copies left behind: {left:?}");
    }
}

fn secret() -> String {
    burrow::secret::Secret::generate().encode()
}

fn write(path: &Path, text: &str) {
    burrow::instance::write_private(path, text).unwrap();
}

const KEEP: &str = if cfg!(windows) { "cmd /c rem" } else { "true" };
const FAIL: &str = if cfg!(windows) {
    "cmd /c exit 1"
} else {
    "false"
};

fn editor_writing(s: &Setup, text: &str) -> String {
    let next = s.locks.path().join("next.yaml");
    std::fs::write(&next, text).unwrap();
    if cfg!(windows) {
        format!("cmd /c copy /y \"{}\"", next.display())
    } else {
        format!("cp \"{}\"", next.display())
    }
}

fn command(s: &Setup, args: &[&str], editor: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_burrow"));
    c.args(args)
        .current_dir(s.dir.path())
        .env("BURROW_INSTANCE_DIR", s.locks.path())
        .env("TMPDIR", s.tmp.path())
        .env("TMP", s.tmp.path())
        .env("TEMP", s.tmp.path())
        .env("EDITOR", editor)
        .env_remove("VISUAL")
        .env_remove("SUDO_EDITOR")
        .stdin(Stdio::null());
    c
}

fn burrow(s: &Setup, args: &[&str], editor: &str) -> Output {
    command(s, args, editor).output().unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn client_yaml(name: &str) -> String {
    format!(
        "server: example.com\nname: {name}\nsecret: {}\nallow: [127.0.0.0/8]\n",
        secret()
    )
}

fn visitor_yaml() -> String {
    format!(
        "server: example.com\nname: laptop\nsecret: {}\nforwards:\n  db: {{ service: db, local: 5432 }}\n",
        secret()
    )
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
}

#[test]
fn a_valid_edit_is_saved_with_the_file_permissions() {
    let s = setup();
    let path = s.dir.path().join("client.yaml");
    let before = client_yaml("home");
    write(&path, &before);
    let after = before.replace("name: home", "name: office");
    let out = burrow(&s, &["edit", "client.yaml"], &editor_writing(&s, &after));
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("saved"), "{}", text(&out));
    assert_eq!(read(&path), after);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let left: Vec<PathBuf> = std::fs::read_dir(s.dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(left, vec![path]);
}

#[test]
fn an_invalid_edit_is_reported_and_never_written() {
    let s = setup();
    let path = s.dir.path().join("client.yaml");
    let before = client_yaml("home");
    write(&path, &before);
    let out = burrow(&s, &["edit", "client"], &editor_writing(&s, "nmae: home\n"));
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let shown = text(&out);
    assert!(shown.contains("nmae"), "{shown}");
    assert!(shown.contains("not saved"), "{shown}");
    assert_eq!(read(&path), before);
}

#[test]
fn closing_the_editor_without_changes_changes_nothing() {
    let s = setup();
    let path = s.dir.path().join("client.yaml");
    let before = client_yaml("home");
    write(&path, &before);
    let out = burrow(&s, &["edit"], KEEP);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("no changes"), "{}", text(&out));
    assert_eq!(read(&path), before);
}

#[test]
fn an_editor_that_fails_changes_nothing() {
    let s = setup();
    let path = s.dir.path().join("client.yaml");
    let before = client_yaml("home");
    write(&path, &before);
    let out = burrow(&s, &["edit"], FAIL);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(text(&out).contains("nothing was changed"), "{}", text(&out));
    assert_eq!(read(&path), before);
}

#[test]
fn a_missing_editor_is_named() {
    let s = setup();
    write(&s.dir.path().join("client.yaml"), &client_yaml("home"));
    let out = burrow(&s, &["edit"], "burrow-no-such-editor");
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(
        text(&out).contains("cannot start the editor burrow-no-such-editor"),
        "{}",
        text(&out)
    );
}

#[test]
fn several_configs_without_a_terminal_are_listed_instead_of_guessed() {
    let s = setup();
    write(&s.dir.path().join("client.yaml"), &client_yaml("home"));
    write(&s.dir.path().join("visitor.yaml"), &visitor_yaml());
    let out = burrow(&s, &["edit"], KEEP);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let shown = text(&out);
    assert!(
        shown.contains("client.yaml") && shown.contains("visitor.yaml"),
        "{shown}"
    );
}

#[test]
fn missing_variables_without_a_terminal_keep_the_file() {
    let s = setup();
    let path = s.dir.path().join("client.yaml");
    let before = "server: example.com\nname: home\nsecret: ${BURROW_EDIT_TEST_UNSET}\nallow: [127.0.0.0/8]\n";
    write(&path, before);
    let after = before.replace("name: home", "name: office");
    let out = command(&s, &["edit"], &editor_writing(&s, &after))
        .env_remove("BURROW_EDIT_TEST_UNSET")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("BURROW_EDIT_TEST_UNSET"),
        "{}",
        text(&out)
    );
    assert_eq!(read(&path), before);
}

#[test]
fn a_file_you_cannot_change_is_refused_before_the_editor_opens() {
    #[cfg(unix)]
    if rustix::process::geteuid().is_root() {
        return;
    }
    let s = setup();
    let path = s.dir.path().join("client.yaml");
    let before = client_yaml("home");
    write(&path, &before);
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&path, perms).unwrap();
    let out = burrow(&s, &["edit", "client.yaml"], &editor_writing(&s, "x\n"));
    let shown = text(&out);
    assert_eq!(out.status.code(), Some(1), "{shown}");
    assert!(shown.contains("you cannot change"), "{shown}");
    assert!(
        !shown.contains("editing "),
        "the editor must not open: {shown}"
    );
    assert_eq!(read(&path), before);
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&path, perms).unwrap();
}

use common::free_port;

#[test]
fn a_running_server_is_found_and_picks_up_the_edit() {
    let s = setup();
    let elsewhere = tempfile::tempdir().unwrap();
    let path = elsewhere.path().join("server.yaml");
    let before = format!(
        "listen: 127.0.0.1:{}\nclients:\n  home:\n    secret: {}\nservices: {{}}\n",
        free_port(),
        secret()
    );
    write(&path, &before);
    let mut server = Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["server", "-c"])
        .arg(&path)
        .env("BURROW_INSTANCE_DIR", s.locks.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let note = || {
        std::fs::read_dir(s.locks.path())
            .unwrap()
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "info"))
    };
    while !note() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let port = free_port();
    let after = before.replace(
        "services: {}",
        &format!(
            "services:\n  web:\n    client: home\n    bind: 127.0.0.1:{port}\n    target: 3000"
        ),
    );
    let out = burrow(&s, &["edit", "server"], &editor_writing(&s, &after));
    let shown = text(&out);
    let _ = server.kill();
    let _ = server.wait();
    assert!(out.status.success(), "{shown}");
    assert!(
        shown.contains(&format!("running, pid {}", server.id())),
        "{shown}"
    );
    assert!(shown.contains("applies it by itself"), "{shown}");
}

#[test]
fn a_running_server_reloads_what_was_saved() {
    let s = setup();
    let elsewhere = tempfile::tempdir().unwrap();
    let path = elsewhere.path().join("server.yaml");
    let before = format!(
        "listen: 127.0.0.1:{}\nclients:\n  home:\n    secret: {}\nservices: {{}}\n",
        free_port(),
        secret()
    );
    write(&path, &before);
    let mut server = Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["server", "-c"])
        .arg(&path)
        .env("BURROW_INSTANCE_DIR", s.locks.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let port = free_port();
    let after = before.replace(
        "services: {}",
        &format!(
            "services:\n  web:\n    client: home\n    bind: 127.0.0.1:{port}\n    target: 3000"
        ),
    );
    let out = burrow(
        &s,
        &["edit", &path.display().to_string()],
        &editor_writing(&s, &after),
    );
    assert!(out.status.success(), "{}", text(&out));
    let start = Instant::now();
    let opened = loop {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break true;
        }
        if start.elapsed() > Duration::from_secs(10) {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = server.kill();
    let _ = server.wait();
    assert!(
        opened,
        "the server did not open the port of the new service"
    );
}

#[cfg(unix)]
#[test]
fn stopping_burrow_while_editing_waits_for_the_editor_and_cleans_up() {
    use std::os::unix::fs::PermissionsExt;
    let s = setup();
    let path = s.dir.path().join("client.yaml");
    let before = client_yaml("home");
    write(&path, &before);
    let next = s.locks.path().join("next.yaml");
    std::fs::write(&next, before.replace("name: home", "name: office")).unwrap();
    let script = s.locks.path().join("slow-editor.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nsleep 1\ncp \"{}\" \"$1\"\n", next.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let child = command(&s, &["edit"], &script.display().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while std::fs::read_dir(s.tmp.path()).unwrap().next().is_none() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "no private copy appeared"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("stopped while editing"),
        "{}",
        text(&out)
    );
    assert_eq!(read(&path), before);
}
