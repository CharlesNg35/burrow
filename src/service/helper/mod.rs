#[cfg(target_os = "macos")]
mod launchd;
#[cfg(target_os = "linux")]
mod systemd;
#[cfg(windows)]
mod windows;

use std::io::Write;
use std::path::PathBuf;

use super::{Backend, Manifest, say};
use crate::base::failure::Failure;
use crate::install::{Platform, Role};

pub const SUFFIX: &str = "-update";
pub const REQUEST: &str = "request.json";
pub const RESULT: &str = "result.json";

pub trait Helper {
    fn install(&self, m: &Manifest, dir: &std::path::Path) -> Result<(), Failure>;
    fn remove(&self, m: &Manifest) -> Result<(), Failure>;
    fn installed(&self, m: &Manifest) -> bool;
    fn fire(&self, _m: &Manifest) -> Result<(), String> {
        Ok(())
    }
    fn request_file(&self, m: &Manifest) -> PathBuf {
        dir(m).join(REQUEST)
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

pub fn dir(m: &Manifest) -> PathBuf {
    let p = Platform::current();
    match p {
        Platform::Linux | Platform::Other => PathBuf::from("/var/lib/burrow").join(m.short()),
        _ => p
            .state_dir()
            .parent()
            .map_or_else(|| p.state_dir(), std::path::Path::to_path_buf)
            .join("updates")
            .join(m.short()),
    }
}

pub fn result_file(m: &Manifest) -> PathBuf {
    dir(m).join(RESULT)
}

pub fn request_file(m: &Manifest) -> Option<PathBuf> {
    helper().map(|h| h.request_file(m))
}

pub fn wanted(m: &Manifest) -> bool {
    m.role == Role::Client && m.remote_updates
}

pub fn set_up(backend: &dyn Backend, m: &Manifest, out: &mut dyn Write) -> Result<(), Failure> {
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
    let dir = dir(m);
    backend.update_folder(m, &dir)?;
    h.install(m, &dir)?;
    say(
        out,
        format!(
            "{}: the server's admin may ask it to update; {} downloads and checks the release, and puts the old one back if the new one does not connect within a minute",
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
