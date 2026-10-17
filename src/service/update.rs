use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    Backend, Manifest, all_manifests, background, parse_version, resolve, save_manifest, say,
    system, this_binary,
};
use crate::failure::Failure;

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
    let mut args: Vec<OsString> = vec![
        "service".into(),
        "update".into(),
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
    background::run(
        "update",
        &format!("updating {}", names.join(", ")),
        args,
        None,
        out,
    )
}

pub fn worker(log: &Path, from: &Path, version: &str, name: Option<&str>, force: bool) -> u8 {
    background::worker(log, |out| swap_all(from, version, name, force, out))
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
