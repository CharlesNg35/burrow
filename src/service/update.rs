use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::{
    Backend, Manifest, Platform, all_manifests, parse_version, resolve, save_manifest, say, system,
    this_binary,
};
use crate::failure::Failure;

const DONE: char = '\u{1}';

struct Swap {
    binary: PathBuf,
    users: Vec<Manifest>,
    installed: String,
}

pub fn update(name: Option<&str>, force: bool, out: &mut dyn Write) -> Result<(), Failure> {
    update_from(&this_binary()?, env!("CARGO_PKG_VERSION"), name, force, out)
}

pub fn update_from(
    me: &Path,
    mine: &str,
    name: Option<&str>,
    force: bool,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    system::backend()?;
    system::require_admin()?;
    let swaps = plan(me, mine, name, force, out)?;
    if swaps.is_empty() {
        return Ok(());
    }
    let log = Platform::current().log_dir().join("update.log");
    std::fs::create_dir_all(log.parent().unwrap_or(Path::new(".")))
        .and_then(|()| std::fs::File::create(&log).map(drop))
        .map_err(|e| Failure::new(format!("cannot create {}: {e}", log.display())))?;
    let names: Vec<&str> = swaps
        .iter()
        .flat_map(|s| s.users.iter().map(|m| m.name.as_str()))
        .collect();
    say(
        out,
        format!(
            "updating {} in the background, so it finishes even if this connection drops (log: {})",
            names.join(", "),
            log.display()
        ),
    )?;
    let mut worker = launch(me, mine, name, force, &log)?;
    follow(&log, &mut worker, out)
}

pub fn worker(log: &Path, from: &Path, version: &str, name: Option<&str>, force: bool) -> u8 {
    let Ok(mut out) = std::fs::OpenOptions::new().append(true).open(log) else {
        return 1;
    };
    let result = swap_all(from, version, name, force, &mut out);
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

fn one_line(s: &str) -> String {
    s.replace(['\n', '\t'], " ")
}

fn swap_all(
    from: &Path,
    version: &str,
    name: Option<&str>,
    force: bool,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let backend = system::backend()?;
    system::require_admin()?;
    for s in plan(from, version, name, force, &mut std::io::sink())? {
        swap(&*backend, s, from, version, out)?;
    }
    Ok(())
}

fn plan(
    me: &Path,
    mine: &str,
    name: Option<&str>,
    force: bool,
    out: &mut dyn Write,
) -> Result<Vec<Swap>, Failure> {
    let targets = match name {
        Some(_) => vec![resolve(name)?],
        None => all_manifests(),
    };
    if targets.is_empty() {
        return Err(Failure::new("no burrow service is installed").hint(format!(
            "install one with: {}",
            crate::failure::admin("service install")
        )));
    }
    let mut binaries: Vec<PathBuf> = targets.iter().map(|m| m.binary.clone()).collect();
    binaries.sort();
    binaries.dedup();
    let mut swaps = Vec::new();
    for binary in binaries {
        let users: Vec<Manifest> = targets
            .iter()
            .filter(|m| m.binary == binary)
            .cloned()
            .collect();
        let names = users
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if !users.iter().all(|m| m.copied_binary) {
            say(
                out,
                format!(
                    "{} is not managed by burrow; upgrade it where you installed it, then run: {}",
                    binary.display(),
                    crate::failure::admin(&format!("service restart {}", users[0].name))
                ),
            )?;
            continue;
        }
        if binary == me {
            say(out, format!("{names}: already uses this binary ({mine})"))?;
            continue;
        }
        let installed = system::binary_version(&binary);
        match (parse_version(mine), parse_version(&installed)) {
            (Some(a), Some(b)) if a == b && !force => {
                say(
                    out,
                    format!("{names}: already on {installed}, nothing to do"),
                )?;
                continue;
            }
            (Some(a), Some(b)) if a < b && !force => {
                say(
                    out,
                    format!(
                        "{names}: installed {installed} is newer than this binary ({mine}); not downgrading (use --force to do it anyway)"
                    ),
                )?;
                continue;
            }
            _ => {}
        }
        swaps.push(Swap {
            binary,
            users,
            installed,
        });
    }
    Ok(swaps)
}

fn swap(
    backend: &dyn Backend,
    s: Swap,
    me: &Path,
    mine: &str,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let Swap {
        binary,
        users,
        installed,
    } = s;
    let names = users
        .iter()
        .map(|m| m.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let running: Vec<&Manifest> = users.iter().filter(|m| backend.status(m).running).collect();
    for m in &running {
        backend.stop(m)?;
    }
    let previous = binary.with_extension("previous");
    let had_old = binary.exists() && system::copy_file(&binary, &previous, true).is_ok();
    if let Err(f) = system::copy_file(me, &binary, true) {
        for m in &running {
            let _ = backend.start(m);
        }
        return Err(f);
    }
    let mut failed = Vec::new();
    for m in &running {
        if backend.start(m).is_err() {
            failed.push(m.name.clone());
        }
    }
    std::thread::sleep(Duration::from_millis(2000));
    for m in &running {
        if !backend.status(m).running && !failed.contains(&m.name) {
            failed.push(m.name.clone());
        }
    }
    if !failed.is_empty() {
        let restored = had_old && system::copy_file(&previous, &binary, true).is_ok();
        if restored {
            for m in &running {
                let _ = backend.stop(m);
                let _ = backend.start(m);
            }
            std::thread::sleep(Duration::from_millis(2000));
        }
        let still_down: Vec<&str> = running
            .iter()
            .filter(|m| !backend.status(m).running)
            .map(|m| m.name.as_str())
            .collect();
        let outcome = match (restored, still_down.is_empty()) {
            (true, true) => format!("restored {installed}, and it runs again"),
            (true, false) => format!(
                "restored {installed}, but {} still does not run",
                still_down.join(", ")
            ),
            (false, _) => format!(
                "could not put {installed} back, so {} is still {mine}",
                binary.display()
            ),
        };
        return Err(Failure::new(format!(
            "{} did not start with {mine}; {outcome}",
            failed.join(", ")
        ))
        .hint(format!(
            "see why with: {}",
            crate::failure::admin(&format!("service logs {}", failed[0]))
        )));
    }
    for m in &users {
        let mut updated = m.clone();
        updated.version = mine.to_string();
        save_manifest(&updated)?;
    }
    say(
        out,
        format!(
            "updated {} {installed} -> {mine}; restarted {names}",
            binary.display()
        ),
    )?;
    if had_old {
        say(
            out,
            format!("  the previous binary is kept at {}", previous.display()),
        )?;
    }
    Ok(())
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

fn launch(
    me: &Path,
    mine: &str,
    name: Option<&str>,
    force: bool,
    log: &Path,
) -> Result<Worker, Failure> {
    let exe = this_binary()?;
    let mut args: Vec<std::ffi::OsString> = vec![
        "service".into(),
        "update".into(),
        "--worker".into(),
        log.into(),
        "--from".into(),
        me.into(),
        "--as-version".into(),
        mine.into(),
    ];
    if let Some(n) = name {
        args.push(n.into());
    }
    if force {
        args.push("--force".into());
    }
    #[cfg(target_os = "linux")]
    {
        let unit = format!("burrow-update-{}", std::process::id());
        let started = Command::new("systemd-run")
            .args(["--quiet", "--collect", "--unit", &unit])
            .arg("--description=burrow update")
            .arg(&exe)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if started {
            return Ok(Worker::Unit(unit));
        }
    }
    spawn(&exe, &args)
        .map(Worker::Child)
        .map_err(|e| Failure::new(format!("cannot start the update: {e}")))
}

#[cfg(unix)]
fn spawn(exe: &Path, args: &[std::ffi::OsString]) -> std::io::Result<std::process::Child> {
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
fn spawn(exe: &Path, args: &[std::ffi::OsString]) -> std::io::Result<std::process::Child> {
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
fn spawn(exe: &Path, args: &[std::ffi::OsString]) -> std::io::Result<std::process::Child> {
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
                Failure::new("the update stopped before it finished").hint(format!(
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
    let message = parts.next().unwrap_or("the update failed").to_string();
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
}
