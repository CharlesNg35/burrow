mod background;
#[cfg(unix)]
mod hand_over;
mod logview;
pub(crate) mod ports;
mod stray;
pub(crate) mod system;
mod update;

mod checks;
mod control;
mod install;
#[cfg(target_os = "macos")]
mod launchd;
mod previous;
#[cfg(target_os = "linux")]
mod systemd;
mod uninstall;
mod web_files;
#[cfg(windows)]
mod windows;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::base::failure::Failure;
use crate::base::output::say;
use crate::install::{Manifest, Platform, all_manifests, load_manifest, manifest_path};

pub use checks::misfits;
pub use control::{Action, control, control_now, list, logs, run_service, status};
pub use install::{InstallOptions, install};
pub use system::{binary_version, copy_file, is_admin};
#[cfg(unix)]
use uninstall::give_history;
pub use uninstall::uninstall;
use uninstall::{admin_history, delete_hint, is_source};
pub use update::{update, update_from, worker as update_worker};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub installed: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub detail: String,
}

pub trait Backend {
    fn kind(&self) -> &'static str;
    fn render(&self, m: &Manifest) -> String;
    fn ensure_account(&self, account: &str) -> Result<bool, Failure>;
    fn remove_account(&self, account: &str) -> Result<(), Failure>;
    fn can_read(&self, account: &str, path: &Path) -> bool;
    fn made_for_burrow(&self, _account: &str) -> bool {
        false
    }
    fn low_ports(&self, _m: &Manifest) -> bool {
        true
    }
    fn wildcard_low_ports(&self) -> bool {
        false
    }
    fn hidden(&self, _file: &Path) -> Option<String> {
        None
    }
    fn cannot_use(&self, m: &Manifest, file: &Path) -> Option<String> {
        if file.is_relative() {
            return Some("a service needs an absolute path".into());
        }
        (!self.can_read(&m.account, file))
            .then(|| format!("its account {} cannot read it", m.account))
    }
    fn secure_file(&self, path: &Path, account: &str) -> Result<(), Failure>;
    fn config_folder(
        &self,
        dir: &Path,
        config: &Path,
        account: &str,
        writable: bool,
    ) -> Result<(), Failure> {
        #[cfg(unix)]
        {
            let owner = if writable { account } else { "root" };
            system::make_dir(dir, if writable { 0o700 } else { 0o755 }, Some(owner))?;
            if let Ok((uid, gid)) = crate::base::process::account_ids(owner) {
                give_history(config, uid, gid);
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (account, config, writable);
            std::fs::create_dir_all(dir)
                .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))
        }
    }
    fn prepare_dirs(&self, account: &str) -> Result<(), Failure>;
    fn state_folder(&self, m: &Manifest) -> Result<Vec<PathBuf>, Failure> {
        let Some(dir) = &m.web_state else {
            return Ok(Vec::new());
        };
        #[cfg(unix)]
        {
            let owner = (m.account != "root").then_some(m.account.as_str());
            system::make_dir(dir, 0o700, owner)?;
            let (uid, gid) = match owner {
                Some(a) => crate::base::process::account_ids(a)?,
                None => (0, 0),
            };
            Ok(hand_over::own_tree(dir, uid, gid))
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(dir)
                .map(|()| Vec::new())
                .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))
        }
    }
    fn write_env(&self, m: &Manifest, env: &BTreeMap<String, String>) -> Result<(), Failure>;
    fn read_env(&self, _m: &Manifest) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
    fn install(&self, m: &Manifest) -> Result<(), Failure>;
    fn uninstall(&self, m: &Manifest) -> Result<(), Failure>;
    fn start(&self, m: &Manifest) -> Result<(), Failure>;
    fn stop(&self, m: &Manifest) -> Result<(), Failure>;
    fn reload(&self, m: &Manifest) -> Result<(), Failure>;
    fn status(&self, m: &Manifest) -> Status;
    fn journal_hint(&self, _m: &Manifest) -> Option<String> {
        None
    }
    fn units(&self) -> Vec<stray::Unit> {
        Vec::new()
    }
    fn manual_stop(&self, name: &str) -> String {
        format!("stop {name} in the service manager")
    }
}

#[cfg(any(target_os = "macos", windows, test))]
pub(super) fn read_json_env(m: &Manifest) -> BTreeMap<String, String> {
    m.env_file
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub(super) fn config_exit_detail(name: &str) -> String {
    format!(
        "the config has errors or cannot be read (exit {}); fix it, then: {}",
        crate::base::failure::EXIT_CONFIG,
        crate::base::failure::admin(&format!("service start {name}"))
    )
}

#[cfg(any(unix, test))]
pub(super) fn is_config_exit(code: &str) -> bool {
    code.parse::<u8>() == Ok(crate::base::failure::EXIT_CONFIG)
}

pub(super) fn logs_hint(name: &str) -> String {
    format!(
        "see why with: {}",
        crate::base::failure::admin(&format!("service logs {name}"))
    )
}

pub fn service_name(config: &Path, given: Option<&str>) -> Result<String, Failure> {
    let raw = match given {
        Some(n) => n.to_string(),
        None => config
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "burrow".into()),
    };
    let clean: String = raw
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean.trim_matches('-').to_string();
    if clean.is_empty() || clean.len() > 48 {
        return Err(
            Failure::usage(format!("\"{raw}\" cannot be used as a service name"))
                .hint("pass --name with 1-48 letters, digits or dashes"),
        );
    }
    let name = crate::install::service_name(&clean);
    let short = crate::install::short_name(&name);
    if OWN_FOLDERS.contains(&short) {
        let source = if given.is_some() {
            "--name"
        } else {
            "the config file name"
        };
        return Err(Failure::usage(format!(
            "\"{short}\" from {source} cannot be used as a service name: it is the name of one of burrow's own folders"
        ))
        .hint("pass --name with another name"));
    }
    Ok(name)
}

const OWN_FOLDERS: [&str; 5] = ["services", "jobs", "logs", "bin", "instances"];

pub fn exposed(m: &Manifest) -> bool {
    let p = Platform::current();
    m.binary == p.binary_path() && !crate::install::trust::binary_trusted(m, p)
}

pub fn exposed_note(m: &Manifest) -> String {
    format!(
        "users other than root can change {}, so burrow does not run or replace {}; move the service to its own copy with: {}",
        m.binary.parent().unwrap_or(&m.binary).display(),
        m.binary.display(),
        crate::base::failure::admin(&install_args(m, &m.config))
    )
}

pub fn recorded_version(m: &Manifest) -> String {
    if exposed(m) {
        format!("not run: {}", exposed_note(m))
    } else {
        system::binary_version(&m.binary)
    }
}

fn save_manifest(m: &Manifest) -> Result<(), Failure> {
    let path = manifest_path(&m.name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))?;
    }
    let text = serde_json::to_string_pretty(m).expect("manifest serializes");
    system::write_file(&path, text.as_bytes())
}

pub struct Installed {
    pub manifest: Manifest,
    pub running: bool,
}

pub fn installed() -> Vec<Installed> {
    let Ok(backend) = system::backend() else {
        return Vec::new();
    };
    all_manifests()
        .into_iter()
        .map(|manifest| Installed {
            running: backend.status(&manifest).running,
            manifest,
        })
        .collect()
}

pub fn env_of(m: &Manifest) -> BTreeMap<String, String> {
    system::backend().map(|b| b.read_env(m)).unwrap_or_default()
}

pub fn saved_env(name: &str) -> BTreeMap<String, String> {
    match (system::backend(), load_manifest(name)) {
        (Ok(b), Ok(m)) => b.read_env(&m),
        _ => BTreeMap::new(),
    }
}

pub fn resolve(name: Option<&str>) -> Result<Manifest, Failure> {
    if let Some(n) = name {
        let full = if n.starts_with("burrow") {
            n.to_string()
        } else {
            format!("burrow-{n}")
        };
        return load_manifest(&full).map_err(|f| stray::named(&full).map_or(f, |s| s.failure()));
    }
    stray::pick(all_manifests(), &stray::strays())
}

pub fn note_strays(out: &mut dyn Write) -> Result<(), Failure> {
    for s in stray::strays() {
        s.note(out)?;
    }
    Ok(())
}

fn p_log_dir() -> PathBuf {
    Platform::current().log_dir()
}

pub fn parse_version(v: &str) -> Option<semver::Version> {
    let v = v.trim().split([' ', '+']).next()?;
    semver::Version::parse(v).ok()
}

pub fn this_binary() -> Result<PathBuf, Failure> {
    let me = std::env::current_exe()
        .map_err(|e| Failure::new(format!("cannot locate this burrow binary: {e}")))?;
    Ok(crate::base::fs::canonical(&me).unwrap_or(me))
}

pub fn as_worker(log: &Path, job: impl FnOnce(&mut dyn Write) -> Result<(), Failure>) -> u8 {
    background::worker(log, job)
}

fn install_args(m: &Manifest, from: &Path) -> String {
    let mut args = format!(
        "service install -c {}",
        crate::base::failure::path_arg(from)
    );
    if from != m.config && service_name(from, None).ok().as_deref() != Some(m.name.as_str()) {
        args.push_str(&format!(" --name {}", m.name));
    }
    if m.admin_writable {
        args.push_str(" --admin-writable");
    }
    args
}

fn writable_args(m: &Manifest) -> String {
    install_args(
        &Manifest {
            admin_writable: true,
            ..m.clone()
        },
        &m.config,
    )
}

fn name_for(source: &Path, given: Option<&str>, installed: &[Manifest]) -> Result<String, Failure> {
    let name = service_name(source, given)?;
    let Some(owner) = installed.iter().find(|m| {
        crate::base::fs::canonical_or_same(&m.config) == crate::base::fs::canonical_or_same(source)
    }) else {
        return Ok(name);
    };
    if given.is_none() || owner.name == name {
        return Ok(owner.name.clone());
    }
    Err(Failure::usage(format!(
        "{} is the installed config of {}, so it cannot be installed as {name}",
        source.display(),
        owner.name
    ))
    .hint(format!(
        "copy it to another folder first, or change {} with: {}",
        owner.name,
        crate::base::failure::admin(&install_args(owner, &owner.config))
    )))
}
#[cfg(test)]
mod tests;
#[cfg(all(test, target_os = "linux"))]
mod tests_web;
