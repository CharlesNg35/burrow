use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    Backend, Manifest, all_manifests, background, parse_version, resolve, save_manifest, say,
    system, this_binary,
};
use crate::base::failure::Failure;

const SETTLE: Duration = if cfg!(test) {
    Duration::from_millis(100)
} else {
    Duration::from_millis(2000)
};

fn steady(backend: &dyn Backend, running: &[&Manifest], failed: &mut Vec<String>) {
    std::thread::sleep(SETTLE);
    let first: Vec<Option<u32>> = running.iter().map(|m| backend.status(m).pid).collect();
    for _ in 0..4 {
        for (m, pid) in running.iter().zip(&first) {
            let now = backend.status(m);
            let restarted = pid.is_some() && now.pid != *pid;
            if (!now.running || restarted) && !failed.contains(&m.name) {
                failed.push(m.name.clone());
            }
        }
        std::thread::sleep(SETTLE);
    }
}

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
    let names: Vec<&str> = swaps
        .iter()
        .flat_map(|s| s.users.iter().map(|m| m.name.as_str()))
        .collect();
    let run = background::Run::new("update")?;
    let from = if this_binary().is_ok_and(|b| b == me) {
        me.to_path_buf()
    } else {
        run.stage(me)?
    };
    let mut args: Vec<OsString> = vec![
        "service".into(),
        "update".into(),
        "--from".into(),
        from.into(),
        "--as-version".into(),
        mine.into(),
    ];
    if let Some(n) = name {
        args.push(n.into());
    }
    if force {
        args.push("--force".into());
    }
    run.start(&format!("updating {}", names.join(", ")), args, None, out)
}

pub fn worker(log: &Path, from: &Path, version: &str, name: Option<&str>, force: bool) -> u8 {
    background::worker(log, |out| {
        if !background::trusted_binary(from) {
            return Err(Failure::usage(format!(
                "{} is not a binary prepared by burrow",
                from.display()
            )));
        }
        swap_all(from, version, name, force, out)
    })
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
    if plan(from, version, name, force, &mut std::io::sink())?.is_empty() {
        return Ok(());
    }
    let _operation = system::operation(out)?;
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
    let all = all_manifests();
    let targets = match name {
        Some(_) => vec![resolve(name)?],
        None => all.clone(),
    };
    if targets.is_empty() {
        return Err(super::stray::none_usable(&super::stray::strays()));
    }
    if name.is_none() {
        super::note_strays(out)?;
    }
    group(&targets, &all, me, mine, force, out)
}

fn group(
    targets: &[Manifest],
    all: &[Manifest],
    me: &Path,
    mine: &str,
    force: bool,
    out: &mut dyn Write,
) -> Result<Vec<Swap>, Failure> {
    let mut binaries: Vec<PathBuf> = targets.iter().map(|m| m.binary.clone()).collect();
    binaries.sort();
    binaries.dedup();
    let mut swaps = Vec::new();
    for binary in binaries {
        let mut users: Vec<Manifest> = all
            .iter()
            .chain(targets)
            .filter(|m| m.binary == binary)
            .cloned()
            .collect();
        users.sort_by(|a, b| a.name.cmp(&b.name));
        users.dedup_by(|a, b| a.name == b.name);
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
                    crate::base::failure::admin(&format!("service restart {}", users[0].name))
                ),
            )?;
            continue;
        }
        if let Some(m) = users.iter().find(|m| super::exposed(m)) {
            say(out, format!("{names}: {}", super::exposed_note(m)))?;
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
    for (i, m) in running.iter().enumerate() {
        if let Err(f) = backend.stop(m) {
            for again in &running[..=i] {
                let _ = backend.start(again);
            }
            return Err(f);
        }
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
    steady(backend, &running, &mut failed);
    if !failed.is_empty() {
        for m in &running {
            let _ = backend.stop(m);
        }
        let restored = had_old && system::copy_file(&previous, &binary, true).is_ok();
        for m in &running {
            let _ = backend.start(m);
        }
        if restored {
            std::thread::sleep(SETTLE);
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
            crate::base::failure::admin(&format!("service logs {}", failed[0]))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::Status;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    struct Fake {
        running: RefCell<BTreeSet<String>>,
        stop_fails: BTreeSet<String>,
        broken: Vec<u8>,
        flaky: Vec<u8>,
        calls: RefCell<u32>,
    }

    impl Backend for Fake {
        fn kind(&self) -> &'static str {
            "fake"
        }
        fn render(&self, _: &Manifest) -> String {
            String::new()
        }
        fn ensure_account(&self, _: &str) -> Result<bool, Failure> {
            Ok(false)
        }
        fn remove_account(&self, _: &str) -> Result<(), Failure> {
            Ok(())
        }
        fn can_read(&self, _: &str, _: &Path) -> bool {
            true
        }
        fn secure_file(&self, _: &Path, _: &str) -> Result<(), Failure> {
            Ok(())
        }
        fn prepare_dirs(&self, _: &str) -> Result<(), Failure> {
            Ok(())
        }
        fn write_env(&self, _: &Manifest, _: &BTreeMap<String, String>) -> Result<(), Failure> {
            Ok(())
        }
        fn install(&self, _: &Manifest) -> Result<(), Failure> {
            Ok(())
        }
        fn uninstall(&self, _: &Manifest) -> Result<(), Failure> {
            Ok(())
        }
        fn start(&self, m: &Manifest) -> Result<(), Failure> {
            if std::fs::read(&m.binary).unwrap() == self.broken {
                return Err(Failure::new("exited at once"));
            }
            self.running.borrow_mut().insert(m.name.clone());
            Ok(())
        }
        fn stop(&self, m: &Manifest) -> Result<(), Failure> {
            self.running.borrow_mut().remove(&m.name);
            if self.stop_fails.contains(&m.name) {
                return Err(Failure::new("did not stop within 10 seconds"));
            }
            Ok(())
        }
        fn reload(&self, _: &Manifest) -> Result<(), Failure> {
            Ok(())
        }
        fn status(&self, m: &Manifest) -> Status {
            let mut calls = self.calls.borrow_mut();
            *calls += 1;
            let flaky = std::fs::read(&m.binary).unwrap() == self.flaky;
            Status {
                installed: true,
                running: self.running.borrow().contains(&m.name),
                pid: Some(if flaky { *calls } else { 1 }),
                detail: String::new(),
            }
        }
    }

    fn service(name: &str, binary: &Path) -> Manifest {
        Manifest {
            name: name.into(),
            role: "client".into(),
            config: PathBuf::from("/etc/burrow/x/client.yaml"),
            source_config: PathBuf::from("/tmp/x.yaml"),
            env_file: None,
            log_file: PathBuf::new(),
            binary: binary.to_path_buf(),
            copied_binary: true,
            account: "burrow".into(),
            created_account: false,
            low_ports: false,
            files: vec![],
            version: "1.0.0".into(),
            admin_writable: false,
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_service_whose_bin_folder_users_can_now_change_is_not_updated_in_place() {
        let usual = crate::install::Platform::current().binary_path();
        crate::install::trust::tests::as_if_users_can_change(usual.parent().unwrap(), || {
            let all = [service("burrow-x", &usual)];
            let mut out = Vec::new();
            let swaps = group(
                &all,
                &all,
                Path::new("/nonexistent/new"),
                "9.9.9",
                true,
                &mut out,
            )
            .unwrap();
            let said = String::from_utf8(out).unwrap();
            assert!(swaps.is_empty(), "{said}");
            assert!(said.contains("users other than root can change"), "{said}");
            assert!(
                said.contains("service install -c /etc/burrow/x/client.yaml"),
                "{said}"
            );
        });
    }

    #[test]
    fn updating_one_service_also_restarts_the_others_that_share_its_binary() {
        let shared = Path::new("/nonexistent/burrow");
        let own = Path::new("/nonexistent/other/burrow");
        let all = [
            service("db", shared),
            service("solo", own),
            service("web", shared),
        ];
        let swaps = group(
            &all[2..],
            &all,
            Path::new("/nonexistent/new"),
            "9.9.9",
            false,
            &mut std::io::sink(),
        )
        .unwrap();
        assert_eq!(swaps.len(), 1);
        let names: Vec<&str> = swaps[0].users.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["db", "web"]);
    }

    fn setup(new: &[u8]) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("burrow");
        let me = dir.path().join("new-burrow");
        std::fs::write(&installed, b"old").unwrap();
        std::fs::write(&me, new).unwrap();
        (dir, installed, me)
    }

    #[test]
    fn a_stop_that_fails_starts_again_what_was_stopped() {
        let (_dir, installed, me) = setup(b"new");
        let users = vec![service("a", &installed), service("b", &installed)];
        let fake = Fake {
            running: RefCell::new(["a".to_string(), "b".to_string()].into()),
            stop_fails: ["b".to_string()].into(),
            broken: b"never".to_vec(),
            flaky: b"never".to_vec(),
            calls: RefCell::new(0),
        };
        let swap_one = Swap {
            binary: installed.clone(),
            users,
            installed: "1.0.0".into(),
        };
        let err = swap(&fake, swap_one, &me, "9.9.9", &mut Vec::new()).unwrap_err();
        assert!(err.message.contains("did not stop"), "{}", err.message);
        assert_eq!(
            *fake.running.borrow(),
            ["a".to_string(), "b".to_string()].into()
        );
        assert_eq!(std::fs::read(&installed).unwrap(), b"old");
    }

    #[test]
    fn a_binary_whose_service_keeps_restarting_is_rolled_back() {
        let (_dir, installed, me) = setup(b"flaky");
        let fake = Fake {
            running: RefCell::new(["a".to_string()].into()),
            stop_fails: BTreeSet::new(),
            broken: b"never".to_vec(),
            flaky: b"flaky".to_vec(),
            calls: RefCell::new(0),
        };
        let swap_one = Swap {
            binary: installed.clone(),
            users: vec![service("a", &installed)],
            installed: "1.0.0".into(),
        };
        let err = swap(&fake, swap_one, &me, "9.9.9", &mut Vec::new()).unwrap_err();
        assert!(err.message.contains("restored 1.0.0"), "{}", err.message);
        assert_eq!(std::fs::read(&installed).unwrap(), b"old");
    }

    #[test]
    fn a_binary_that_does_not_start_is_rolled_back_and_the_services_run_again() {
        let (_dir, installed, me) = setup(b"broken");
        let fake = Fake {
            running: RefCell::new(["a".to_string()].into()),
            stop_fails: BTreeSet::new(),
            broken: b"broken".to_vec(),
            flaky: b"never".to_vec(),
            calls: RefCell::new(0),
        };
        let swap_one = Swap {
            binary: installed.clone(),
            users: vec![service("a", &installed)],
            installed: "1.0.0".into(),
        };
        let err = swap(&fake, swap_one, &me, "9.9.9", &mut Vec::new()).unwrap_err();
        assert!(
            err.message.contains("restored 1.0.0, and it runs again"),
            "{}",
            err.message
        );
        assert_eq!(std::fs::read(&installed).unwrap(), b"old");
        assert!(fake.running.borrow().contains("a"));
    }
}
