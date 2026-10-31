use std::collections::VecDeque;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::{tools, ui};

pub const DEFAULT_CAP: usize = 6;
const FIRST: usize = 2;
const STEP: Duration = Duration::from_secs(3);
const BUSY_SHARE: f64 = 0.6;
const SAFE_MB: u64 = 6 * 1024;
const MB_PER_FLOW: u64 = 1536;

pub fn shots_dir(root: &Path) -> PathBuf {
    std::env::var_os("BURROW_E2E_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("tmp/admin-ui-shots"))
}

pub fn build_args() -> Vec<String> {
    [
        "test",
        "--locked",
        "--features",
        "admin-test-hooks",
        "--test",
        "admin_e2e",
        "--no-run",
        "--message-format=json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

pub fn flow_args(name: &str) -> Vec<String> {
    vec![
        name.to_string(),
        "--exact".to_string(),
        "--test-threads=1".to_string(),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Load {
    pub load1: f64,
    pub cores: usize,
    pub available_mb: u64,
}

pub fn may_start(running: usize, cap: usize, load: Option<Load>) -> Result<(), String> {
    if running >= cap {
        return Err(format!("at the cap of {cap}"));
    }
    let Some(l) = load else {
        return Ok(());
    };
    if running == 0 {
        return Ok(());
    }
    let busy = l.load1 >= BUSY_SHARE * l.cores as f64;
    if busy {
        return Err(format!("load {:.1}/{} is high", l.load1, l.cores));
    }
    let need = SAFE_MB + MB_PER_FLOW * (running as u64 + 1);
    if l.available_mb < need {
        return Err(format!(
            "{} MB available, {need} MB needed for one more",
            l.available_mb
        ));
    }
    Ok(())
}

fn measure() -> Option<Load> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let load1: f64 = std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = info
        .lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(Load {
        load1,
        cores: std::thread::available_parallelism().map_or(1, |n| n.get()),
        available_mb: kb / 1024,
    })
}

fn describe(running: usize, load: Option<Load>) -> String {
    match load {
        Some(l) => format!(
            "running {running}, load {:.1}/{}, {} GB free",
            l.load1,
            l.cores,
            l.available_mb / 1024
        ),
        None => format!("running {running}"),
    }
}

fn parse_jobs(text: &str) -> Result<usize, String> {
    text.parse::<usize>()
        .ok()
        .filter(|n| *n >= 1)
        .ok_or_else(|| format!("--jobs takes a number of at least 1, not {text:?}"))
}

pub fn flows_in(list: &str, filter: Option<&str>) -> Vec<String> {
    list.lines()
        .filter_map(|l| l.strip_suffix(": test"))
        .filter(|name| filter.is_none_or(|f| name.contains(f)))
        .map(str::to_string)
        .collect()
}

fn test_binary(root: &Path) -> Result<PathBuf, String> {
    let out = crate::cargo()
        .current_dir(root)
        .args(build_args())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cargo test --no-run: {e}"))?;
    if !out.status.success() {
        return Err("the admin e2e flows did not build".into());
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == "admin_e2e")
        .find_map(|m| m["executable"].as_str().map(PathBuf::from))
        .ok_or_else(|| "cargo did not report the admin_e2e test binary".into())
}

fn start_fresh(shots: &Path) -> Result<(), String> {
    if let Ok(entries) = std::fs::read_dir(shots) {
        for e in entries.flatten() {
            let p = e.path();
            let _ = if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                std::fs::remove_file(&p)
            };
        }
    }
    std::fs::create_dir_all(shots).map_err(|e| format!("{}: {e}", shots.display()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    std::fs::write(shots.join("full-run.txt"), now.to_string())
        .map_err(|e| format!("{}: {e}", shots.display()))
}

struct Running {
    name: String,
    child: Child,
    log: PathBuf,
    since: Instant,
}

const USAGE: &str = "usage: cargo xtask e2e [flow-filter] [--jobs N] [--update-baselines]";

pub fn main(root: &Path, args: &[String]) -> Result<(), String> {
    let (update, filter, asked) = parse_args(args)?;
    let cap = match (update, asked) {
        (true, _) => 1,
        (false, Some(n)) => n,
        (false, None) if measure().is_some() => DEFAULT_CAP,
        (false, None) => 1,
    };
    if std::env::var_os("BURROW_E2E_CHROME").is_none()
        && std::env::var("BURROW_E2E_SYSTEM_CHROME").as_deref() != Ok("1")
    {
        let chrome = tools::chrome(root)?;
        println!("Chrome for Testing {}: {}", tools::CHROME, chrome.display());
    }
    ui::main(root, &["--release".to_string()])?;
    let exe = test_binary(root)?;
    let listed = Command::new(&exe)
        .arg("--list")
        .output()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    let mut pending: VecDeque<String> =
        flows_in(&String::from_utf8_lossy(&listed.stdout), filter).into();
    if pending.is_empty() {
        return Err(format!("no flow matches {}", filter.unwrap_or("")));
    }
    let shots = shots_dir(root);
    if filter.is_none() {
        start_fresh(&shots)?;
    }
    let logs = shots.join("logs");
    std::fs::create_dir_all(&logs).map_err(|e| format!("{}: {e}", logs.display()))?;
    println!(
        "e2e: {} flows, at most {cap} at a time{}",
        pending.len(),
        if update {
            " (recording baselines runs one at a time)"
        } else {
            ""
        }
    );
    let started = Instant::now();
    let mut running: Vec<Running> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    let mut peak = 0;
    let mut started_flows = 0;
    let mut last_check = Instant::now() - STEP;
    let mut waiting_for = String::new();
    while !pending.is_empty() || !running.is_empty() {
        reap(&mut running, &mut failed)?;
        let first = running.is_empty() && peak == 0;
        if !pending.is_empty() && (first || last_check.elapsed() >= STEP) {
            last_check = Instant::now();
            let load = measure();
            let burst = if first { FIRST.min(cap) } else { 1 };
            for _ in 0..burst {
                let Some(name) = pending.front().cloned() else {
                    break;
                };
                let decision = if first && running.len() < burst {
                    may_start(running.len(), cap, None)
                } else {
                    may_start(running.len(), cap, load)
                };
                match decision {
                    Ok(()) => {
                        pending.pop_front();
                        println!("{}: starting {name}", describe(running.len(), load));
                        running.push(spawn_flow(root, &exe, &logs, name, update)?);
                        peak = peak.max(running.len());
                        started_flows += 1;
                        waiting_for.clear();
                    }
                    Err(why) => {
                        if why != waiting_for {
                            println!("{}: waiting, {why}", describe(running.len(), load));
                            waiting_for = why;
                        }
                        break;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let total = started.elapsed().as_secs();
    println!(
        "e2e: {} passed, {} failed in {}m{:02}s, at most {peak} at a time",
        started_flows - failed.len(),
        failed.len(),
        total / 60,
        total % 60
    );
    println!("contact sheet: {}", shots.join("index.html").display());
    if !failed.is_empty() {
        return Err(format!("the admin e2e flows failed: {}", failed.join(", ")));
    }
    Ok(())
}

fn parse_args(args: &[String]) -> Result<(bool, Option<&str>, Option<usize>), String> {
    let mut update = false;
    let mut filter = None;
    let mut asked = None;
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--update-baselines" => update = true,
            "--jobs" => {
                let n = rest.next().ok_or_else(|| USAGE.to_string())?;
                asked = Some(parse_jobs(n)?);
            }
            other if other.starts_with("--jobs=") => {
                asked = Some(parse_jobs(&other["--jobs=".len()..])?);
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other}; {USAGE}"));
            }
            other => filter = Some(other),
        }
    }
    if asked.is_none()
        && let Ok(n) = std::env::var("E2E_JOBS")
        && !n.trim().is_empty()
    {
        asked = Some(parse_jobs(n.trim())?);
    }
    Ok((update, filter, asked))
}

fn reap(running: &mut Vec<Running>, failed: &mut Vec<String>) -> Result<(), String> {
    let mut i = 0;
    while i < running.len() {
        match running[i].child.try_wait() {
            Ok(Some(status)) => {
                let r = running.swap_remove(i);
                let secs = r.since.elapsed().as_secs();
                if status.success() {
                    println!("ok      {} ({secs} s)", r.name);
                } else {
                    println!("FAILED  {} ({secs} s), log: {}", r.name, r.log.display());
                    let text = std::fs::read_to_string(&r.log).unwrap_or_default();
                    let tail: Vec<&str> = text.lines().rev().take(25).collect();
                    for l in tail.into_iter().rev() {
                        println!("        {l}");
                    }
                    failed.push(r.name);
                }
            }
            Ok(None) => i += 1,
            Err(e) => return Err(format!("waiting for {}: {e}", running[i].name)),
        }
    }

    Ok(())
}

fn spawn_flow(
    root: &Path,
    exe: &Path,
    logs: &Path,
    name: String,
    update: bool,
) -> Result<Running, String> {
    let log = logs.join(format!("{}.log", name.replace("::", "-")));
    let out = File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.current_dir(root)
        .args(flow_args(&name))
        .env_remove("BURROW_E2E_FRESH")
        .stdout(out)
        .stderr(err);
    if update {
        cmd.env("BURROW_E2E_UPDATE_BASELINES", "1");
    }
    let child = cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))?;
    Ok(Running {
        name,
        child,
        log,
        since: Instant::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_flow_runs_alone_in_its_own_process() {
        let a = flow_args("edits::e3");
        assert_eq!(a, ["edits::e3", "--exact", "--test-threads=1"]);
        assert!(build_args().contains(&"admin-test-hooks".to_string()));
        assert!(build_args().contains(&"--no-run".to_string()));
    }

    #[test]
    fn the_flows_come_from_the_test_list() {
        let list = "edits::e3_add: test\nsecrets::e2_new: test\nchecks::tests::x: test\n\n3 tests, 0 benchmarks\n";
        assert_eq!(flows_in(list, None).len(), 3);
        assert_eq!(flows_in(list, Some("e2")), ["secrets::e2_new"]);
    }

    #[test]
    fn more_flows_start_only_while_cpu_and_memory_allow() {
        let l = |load1: f64, gb: u64| {
            Some(Load {
                load1,
                cores: 10,
                available_mb: gb * 1024,
            })
        };
        assert!(may_start(0, 6, l(9.0, 1)).is_ok());
        assert!(may_start(2, 6, l(2.0, 20)).is_ok());
        assert!(may_start(2, 6, l(6.5, 20)).is_err());
        assert!(may_start(3, 6, l(2.0, 11)).is_err());
        assert!(may_start(3, 6, l(2.0, 13)).is_ok());
        assert!(may_start(6, 6, l(0.0, 64)).is_err());
        assert!(may_start(1, 1, None).is_err());
        assert!(may_start(3, 6, None).is_ok());
        assert_eq!(parse_jobs("2"), Ok(2));
        assert!(parse_jobs("0").is_err());
        assert!(parse_jobs("x").is_err());
    }
}
