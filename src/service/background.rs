use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::{Platform, say, this_binary};
use crate::failure::Failure;

const DONE: char = '\u{1}';

pub fn run(
    job: &str,
    what: &str,
    mut args: Vec<OsString>,
    secrets: Option<&BTreeMap<String, String>>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let log = Platform::current().log_dir().join(format!("{job}.log"));
    std::fs::create_dir_all(log.parent().unwrap_or(Path::new(".")))
        .and_then(|()| std::fs::File::create(&log).map(drop))
        .map_err(|e| Failure::new(format!("cannot create {}: {e}", log.display())))?;
    if let Some(env) = secrets.filter(|e| !e.is_empty()) {
        let text = serde_json::to_string(env).expect("variables serialize");
        crate::instance::write_private(&secrets_of(&log), &text)
            .map_err(|e| Failure::new(format!("cannot hand the variables to the {job}: {e}")))?;
    }
    args.push("--worker".into());
    args.push(log.clone().into());
    say(
        out,
        format!(
            "{what} in the background, so it finishes even if this connection drops (log: {})",
            log.display()
        ),
    )?;
    let mut worker = launch(job, &args)?;
    follow(&log, &mut worker, out)
}

pub fn worker(log: &Path, job: impl FnOnce(&mut dyn Write) -> Result<(), Failure>) -> u8 {
    let Ok(mut out) = std::fs::OpenOptions::new().append(true).open(log) else {
        return 1;
    };
    let result = job(&mut out);
    let (line, code) = match &result {
        Ok(()) => (format!("{DONE}ok"), 0),
        Err(f) => (
            format!(
                "{DONE}{}\t{}\t{}",
                f.exit,
                one_line(&f.message),
                f.hint.as_deref().map(one_line).unwrap_or_default()
            ),
            f.exit.max(1),
        ),
    };
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
    code
}

pub fn take_secrets(log: &Path) -> BTreeMap<String, String> {
    let path = secrets_of(log);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    serde_json::from_str(&text).unwrap_or_default()
}

fn secrets_of(log: &Path) -> PathBuf {
    log.with_extension("env")
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\t'], " ")
}

enum Worker {
    Child(std::process::Child),
    #[cfg(target_os = "linux")]
    Unit(String),
}

impl Worker {
    fn running(&mut self) -> bool {
        match self {
            Worker::Child(c) => matches!(c.try_wait(), Ok(None)),
            #[cfg(target_os = "linux")]
            Worker::Unit(unit) => Command::new("systemctl")
                .args(["is-active", "--quiet", unit])
                .status()
                .is_ok_and(|s| s.success()),
        }
    }
}

fn launch(job: &str, args: &[OsString]) -> Result<Worker, Failure> {
    let exe = this_binary()?;
    #[cfg(target_os = "linux")]
    {
        let unit = format!("burrow-{job}-{}", std::process::id());
        let started = Command::new("systemd-run")
            .args(["--quiet", "--collect", "--unit", &unit])
            .arg(format!("--description=burrow {job}"))
            .arg(&exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if started {
            return Ok(Worker::Unit(unit));
        }
    }
    spawn(&exe, args)
        .map(Worker::Child)
        .map_err(|e| Failure::new(format!("cannot start the {job}: {e}")))
}

#[cfg(unix)]
fn spawn(exe: &Path, args: &[OsString]) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
}

#[cfg(windows)]
fn spawn(exe: &Path, args: &[OsString]) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let command = |flags: u32| {
        Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    command(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| command(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP))
}

#[cfg(not(any(unix, windows)))]
fn spawn(exe: &Path, args: &[OsString]) -> std::io::Result<std::process::Child> {
    Command::new(exe).args(args).spawn()
}

fn follow(log: &Path, worker: &mut Worker, out: &mut dyn Write) -> Result<(), Failure> {
    let mut offset = 0u64;
    let mut pending = String::new();
    loop {
        let alive = worker.running();
        if let Ok(mut f) = std::fs::File::open(log)
            && f.seek(SeekFrom::Start(offset)).is_ok()
        {
            let mut chunk = String::new();
            if let Ok(n) = f.read_to_string(&mut chunk) {
                offset += n as u64;
                pending.push_str(&chunk);
            }
        }
        while let Some(end) = pending.find('\n') {
            let line: String = pending.drain(..=end).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            if let Some(result) = line.strip_prefix(DONE) {
                return finished(result);
            }
            say(out, line)?;
        }
        if !alive {
            return Err(
                Failure::new("the background job stopped before it finished").hint(format!(
                    "see {} and {}",
                    log.display(),
                    crate::failure::admin("service status")
                )),
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn finished(result: &str) -> Result<(), Failure> {
    if result == "ok" {
        return Ok(());
    }
    let mut parts = result.splitn(3, '\t');
    let exit = parts.next().and_then(|e| e.parse().ok()).unwrap_or(1);
    let message = parts
        .next()
        .unwrap_or("the background job failed")
        .to_string();
    let hint = parts.next().filter(|h| !h.is_empty());
    let mut f = Failure::new(message);
    f.exit = exit;
    Err(match hint {
        Some(h) => f.hint(h),
        None => f,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_failure_comes_back_with_its_hint_and_exit_code() {
        let f = finished("3\tweb did not start with 9.9.9; restored 1.0.0\tsee why with: sudo burrow service logs web")
            .unwrap_err();
        assert_eq!(f.message, "web did not start with 9.9.9; restored 1.0.0");
        assert_eq!(
            f.hint.as_deref(),
            Some("see why with: sudo burrow service logs web")
        );
        assert_eq!(f.exit, 3);
        let f = finished("1\tno burrow service is installed\t").unwrap_err();
        assert_eq!(f.hint, None);
        assert!(finished("ok").is_ok());
    }

    #[test]
    fn messages_stay_on_one_line() {
        assert_eq!(one_line("a\nb\tc"), "a b c");
    }

    #[test]
    fn the_worker_reports_its_result_and_variables_are_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("job.log");
        std::fs::File::create(&log).unwrap();
        let env = BTreeMap::from([("HOME_SECRET".to_string(), "s3cret".to_string())]);
        crate::instance::write_private(&secrets_of(&log), &serde_json::to_string(&env).unwrap())
            .unwrap();
        let code = worker(&log, |out| {
            assert_eq!(take_secrets(&log), env);
            writeln!(out, "step one").unwrap();
            Err(Failure::new("broke\nbadly").hint("try again"))
        });
        assert_eq!(code, 1);
        assert!(!secrets_of(&log).exists());
        assert!(take_secrets(&log).is_empty());
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text, "step one\n\u{1}1\tbroke badly\ttry again\n");
    }
}
