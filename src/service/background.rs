use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::{say, this_binary};
use crate::base::failure::Failure;
use crate::install::Platform;

const DONE: char = '\u{1}';

const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

pub struct Run {
    dir: PathBuf,
    id: String,
    log: PathBuf,
}

impl Run {
    pub fn new(job: &str) -> Result<Run, Failure> {
        let dir = Platform::current().jobs_dir();
        crate::instance::private_dir(&dir)
            .map_err(|e| Failure::new(format!("cannot prepare {}: {e}", dir.display())))?;
        sweep(&dir);
        let id = format!("{job}-{}", std::process::id());
        let log = dir.join(format!("{id}.log"));
        std::fs::File::create(&log)
            .map_err(|e| Failure::new(format!("cannot create {}: {e}", log.display())))?;
        Ok(Run { dir, id, log })
    }

    pub fn stage(&self, binary: &Path) -> Result<PathBuf, Failure> {
        let staged = self.dir.join(format!("{}.bin", self.id));
        super::system::copy_file(binary, &staged, true)?;
        Ok(staged)
    }

    pub fn start(
        self,
        what: &str,
        mut args: Vec<OsString>,
        secrets: Option<&BTreeMap<String, String>>,
        out: &mut dyn Write,
    ) -> Result<(), Failure> {
        if let Some(env) = secrets.filter(|e| !e.is_empty()) {
            let text = serde_json::to_string(env).expect("variables serialize");
            if let Err(e) = crate::base::fs::write_private(&secrets_of(&self.log), &text) {
                self.remove();
                return Err(Failure::new(format!(
                    "cannot hand the variables to the background job: {e}"
                )));
            }
        }
        args.push("--worker".into());
        args.push(self.log.clone().into());
        let mut worker = match launch(&self.id, &args) {
            Ok(w) => w,
            Err(f) => {
                self.remove();
                return Err(f);
            }
        };
        say(
            out,
            format!(
                "{what} in the background, so it finishes even if this connection drops (log: {})",
                self.log.display()
            ),
        )?;
        let result = follow(&self.log, &mut worker, out)?;
        self.remove();
        result
    }

    fn remove(&self) {
        let _ = std::fs::remove_file(&self.log);
        let _ = std::fs::remove_file(secrets_of(&self.log));
        let _ = std::fs::remove_file(self.dir.join(format!("{}.bin", self.id)));
    }
}

fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() == super::system::OPERATION_LOCK {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > KEEP);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn in_jobs_dir(path: &Path) -> bool {
    let dir = Platform::current().jobs_dir();
    let canonical = |p: &Path| std::fs::canonicalize(p).ok();
    match (path.parent().and_then(canonical), canonical(&dir)) {
        (Some(parent), Some(dir)) => parent == dir,
        _ => false,
    }
}

pub fn trusted_binary(from: &Path) -> bool {
    this_binary().is_ok_and(|me| me == from) || in_jobs_dir(from)
}

pub fn worker(log: &Path, job: impl FnOnce(&mut dyn Write) -> Result<(), Failure>) -> u8 {
    if !in_jobs_dir(log) || log.extension().is_none_or(|e| e != "log") {
        return crate::base::failure::EXIT_USAGE;
    }
    run_job(log, job)
}

fn run_job(log: &Path, job: impl FnOnce(&mut dyn Write) -> Result<(), Failure>) -> u8 {
    let Ok(mut out) = std::fs::OpenOptions::new().append(true).open(log) else {
        return 1;
    };
    let result = job(&mut out);
    let _ = std::fs::remove_file(secrets_of(log));
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
    if !in_jobs_dir(log) {
        return BTreeMap::new();
    }
    read_secrets(log)
}

fn read_secrets(log: &Path) -> BTreeMap<String, String> {
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

fn launch(id: &str, args: &[OsString]) -> Result<Worker, Failure> {
    let exe = this_binary()?;
    #[cfg(target_os = "linux")]
    {
        let unit = format!("{}{id}", crate::install::SERVICE_PREFIX);
        let started = Command::new("systemd-run")
            .args(["--quiet", "--collect", "--unit", &unit])
            .arg(format!("--description=burrow {id}"))
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
        .map_err(|e| Failure::new(format!("cannot start the background job {id}: {e}")))
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

fn follow(
    log: &Path,
    worker: &mut Worker,
    out: &mut dyn Write,
) -> Result<Result<(), Failure>, Failure> {
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
                return Ok(finished(result));
            }
            say(out, line)?;
        }
        if !alive {
            return Err(
                Failure::new("the background job stopped before it finished").hint(format!(
                    "see {} and {}",
                    log.display(),
                    crate::base::failure::admin("service status")
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
        crate::base::fs::write_private(&secrets_of(&log), &serde_json::to_string(&env).unwrap())
            .unwrap();
        let code = run_job(&log, |out| {
            assert_eq!(read_secrets(&log), env);
            writeln!(out, "step one").unwrap();
            Err(Failure::new("broke\nbadly").hint("try again"))
        });
        assert_eq!(code, 1);
        assert!(!secrets_of(&log).exists());
        assert!(read_secrets(&log).is_empty());
        assert!(take_secrets(&log).is_empty());
        assert_eq!(worker(&log, |_| Ok(())), crate::base::failure::EXIT_USAGE);
        assert!(!trusted_binary(&log));
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text, "step one\n\u{1}1\tbroke badly\ttry again\n");
    }
}
