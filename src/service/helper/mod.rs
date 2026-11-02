//! The root (SYSTEM on Windows) updater beside a client service: install, trigger and result files per service manager.

pub mod acl;
#[cfg(target_os = "macos")]
mod launchd;
#[cfg(target_os = "linux")]
mod systemd;
#[cfg(windows)]
mod windows;

use std::io::Write;
use std::path::PathBuf;

use super::{Manifest, say};
use crate::base::failure::Failure;
use crate::install::{Platform, Role};

pub const SUFFIX: &str = "-update";
pub const TRIGGER: &str = "update-now";
pub const RESULT: &str = "result.json";

pub trait Helper {
    fn install(&self, m: &Manifest) -> Result<(), Failure>;
    fn remove(&self, m: &Manifest) -> Result<(), Failure>;
    fn installed(&self, m: &Manifest) -> bool;
    fn trigger(&self, m: &Manifest) -> Option<PathBuf>;
    fn fire(&self, m: &Manifest) -> Result<(), String> {
        let file = self
            .trigger(m)
            .ok_or("this system has no updater to start")?;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file)
            .map(drop)
            .map_err(|e| format!("cannot write {}: {e}", file.display()))
    }
    fn take_trigger(&self, m: &Manifest) {
        #[cfg(unix)]
        if let Some(file) = self.trigger(m)
            && let Some(dir) = file.parent()
        {
            super::nofollow::remove_entry(dir, TRIGGER);
        }
        #[cfg(not(unix))]
        let _ = m;
    }
}

pub fn helper() -> Option<Box<dyn Helper>> {
    #[cfg(target_os = "linux")]
    {
        Some(Box::new(systemd::PathUnit))
    }
    #[cfg(target_os = "macos")]
    {
        Some(Box::new(launchd::QueueJob))
    }
    #[cfg(windows)]
    {
        Some(Box::new(windows::StartableService))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

pub fn helper_name(m: &Manifest) -> String {
    format!("{}{SUFFIX}", m.name)
}

fn base() -> PathBuf {
    let p = Platform::current();
    match p {
        Platform::Linux | Platform::Other => PathBuf::from("/var/lib/burrow"),
        _ => p
            .state_dir()
            .parent()
            .map_or_else(|| p.state_dir(), std::path::Path::to_path_buf),
    }
}

pub fn dir(m: &Manifest) -> PathBuf {
    base().join("updates").join(m.short())
}

pub fn own_dir(m: &Manifest) -> PathBuf {
    match Platform::current() {
        Platform::Linux | Platform::Other => base().join(m.short()),
        _ => dir(m).join("own"),
    }
}

pub fn result_file(m: &Manifest) -> PathBuf {
    dir(m).join(RESULT)
}

pub fn trigger(m: &Manifest) -> Option<PathBuf> {
    helper()?.trigger(m)
}

pub fn wanted(m: &Manifest) -> bool {
    m.role == Role::Client && m.remote_updates
}

fn results_folder(m: &Manifest) -> Result<(), Failure> {
    #[cfg(unix)]
    {
        let _ = m;
        let base = base();
        std::fs::create_dir_all(&base)
            .map_err(|e| Failure::new(format!("cannot create {}: {e}", base.display())))?;
        super::nofollow::subfolder(&base, "updates", 0o755, None)?;
        super::nofollow::subfolder(&base.join("updates"), m.short(), 0o755, None)?;
        if cfg!(target_os = "macos") {
            let owner = match m.account.as_str() {
                "root" => None,
                a => Some(crate::base::process::account_ids(a)?),
            };
            super::nofollow::subfolder(&dir(m), "own", 0o700, owner)?;
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        super::windows::results_folder(&dir(m), &m.account)?;
        super::windows::own_folder(&own_dir(m), &m.account)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = m;
        Ok(())
    }
}

pub fn set_up(m: &Manifest, out: &mut dyn Write) -> Result<(), Failure> {
    let Some(h) = helper() else {
        return Ok(());
    };
    if !wanted(m) {
        if h.installed(m) {
            remove_with(&*h, m)?;
            say(
                out,
                format!("{}: remote updates are off; removed its updater", m.name),
            )?;
        }
        return Ok(());
    }
    results_folder(m)?;
    h.install(m)?;
    say(
        out,
        format!(
            "{}: the server's admin may ask it to update to the latest release; {} downloads and checks it, and puts the old one back if the new one does not connect within a minute",
            m.name,
            helper_name(m)
        ),
    )
}

pub fn remove(m: &Manifest) -> Result<(), Failure> {
    match helper() {
        Some(h) if h.installed(m) => remove_with(&*h, m),
        _ => Ok(()),
    }
}

fn remove_with(h: &dyn Helper, m: &Manifest) -> Result<(), Failure> {
    h.remove(m)?;
    h.take_trigger(m);
    if m.role == Role::Client {
        let _ = std::fs::remove_dir_all(dir(m));
    }
    Ok(())
}

pub fn fire(m: &Manifest) -> Result<(), String> {
    match helper() {
        Some(h) => h.fire(m),
        None => Err("this system has no service manager burrow can use".into()),
    }
}

pub fn take_trigger(m: &Manifest) {
    if let Some(h) = helper() {
        h.take_trigger(m);
    }
}
