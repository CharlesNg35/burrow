use std::os::unix::fs::PermissionsExt;
use std::time::Instant;

use super::super::wire::name;
use super::*;

fn script(dir: &tempfile::TempDir, body: &str) -> PathBuf {
    let path = dir.path().join("hook.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[tokio::test]
async fn the_hook_gets_lego_s_arguments_and_only_path() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log");
    let hook = script(
        &dir,
        &format!("echo \"$@\" >> {0}\nenv | sort >> {0}", log.display()),
    );
    let c = Command::new(hook);
    let n = name("_acme-challenge.Erp.Acme.example.com").unwrap();
    c.present(&n, "value-1").await.unwrap();
    c.cleanup(&n, "value-1").await.unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "present _acme-challenge.erp.acme.example.com. value-1"
    );
    let env: Vec<&str> = lines[1..]
        .iter()
        .copied()
        .take_while(|l| !l.starts_with("cleanup"))
        .filter(|l| !l.starts_with("PWD=") && !l.starts_with("SHLVL=") && !l.starts_with("_="))
        .collect();
    assert_eq!(env.len(), 1, "{env:?}");
    assert!(env[0].starts_with("PATH="), "{env:?}");
    assert!(text.contains("cleanup _acme-challenge.erp.acme.example.com. value-1"));
}

#[tokio::test]
async fn a_failing_hook_shows_its_error_output() {
    let dir = tempfile::tempdir().unwrap();
    let hook = script(
        &dir,
        "echo starting\necho 'no token for this zone' >&2\nexit 3",
    );
    let err = Command::new(hook.clone())
        .present(&name("_acme-challenge.a.example.com").unwrap(), "v")
        .await
        .unwrap_err();
    assert_eq!(
        err.text(),
        format!(
            "{} present failed (exit status: 3): no token for this zone",
            hook.display()
        )
    );
}

#[tokio::test]
async fn a_hook_that_hangs_is_stopped_at_its_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let hook = script(&dir, "sleep 30");
    let start = Instant::now();
    let err = Command::new(hook)
        .with_timeout(Duration::from_millis(300))
        .cleanup(&name("_acme-challenge.a.example.com").unwrap(), "v")
        .await
        .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(
        err.text()
            .ends_with("cleanup did not finish within 0 s and was stopped"),
        "{}",
        err.text()
    );
    let missing = Command::new(dir.path().join("nope"))
        .present(&name("_acme-challenge.a.example.com").unwrap(), "v")
        .await
        .unwrap_err();
    assert!(
        missing.text().starts_with("cannot run "),
        "{}",
        missing.text()
    );
}

#[test]
fn long_output_keeps_its_last_part_on_one_line() {
    let long = format!("first\n{}\nlast words", "x".repeat(400));
    let t = tail(&long);
    assert_eq!(t.chars().count(), 300);
    assert!(t.ends_with("x last words"));
}
