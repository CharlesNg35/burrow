//! Records which burrow services are installed: their manifests, names and folders on each platform.

mod platform;
mod role;
mod state;
pub(crate) mod trust;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::base::failure::Failure;
use trust::binary_trusted;

pub use platform::Platform;
pub use role::Role;
pub use state::{
    choose_state_dir, missing_state_folder, prunes_uploads_in, service_state_dir, state_dir_for,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub role: Role,
    pub config: PathBuf,
    pub source_config: PathBuf,
    pub env_file: Option<PathBuf>,
    #[serde(default)]
    pub log_file: PathBuf,
    pub binary: PathBuf,
    pub copied_binary: bool,
    pub account: String,
    pub created_account: bool,
    pub low_ports: bool,
    #[serde(default)]
    pub files: Vec<PathBuf>,
    pub version: String,
    pub admin_writable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_state: Option<PathBuf>,
}

impl Manifest {
    pub fn short(&self) -> &str {
        short_name(&self.name)
    }

    pub fn folder(&self) -> &Path {
        self.config.parent().unwrap_or(&self.config)
    }

    pub fn command(&self) -> &str {
        self.role.command()
    }
}

pub const SERVICE_PREFIX: &str = "burrow-";

pub fn is_service_name(name: &str) -> bool {
    name == "burrow" || name.starts_with(SERVICE_PREFIX)
}

pub fn service_name(short: &str) -> String {
    match is_service_name(short) {
        true => short.to_string(),
        false => format!("{SERVICE_PREFIX}{short}"),
    }
}

pub fn short_name(name: &str) -> &str {
    name.strip_prefix(SERVICE_PREFIX).unwrap_or(name)
}

pub fn manifest_path(name: &str) -> PathBuf {
    Platform::current().state_dir().join(format!("{name}.json"))
}

pub(crate) fn trustworthy(m: &Manifest, file: &Path) -> bool {
    let p = Platform::current();
    let in_dir = |path: &Path, dir: &Path| path.parent() == Some(dir);
    let own_dir = p.config_dir().join(m.short());
    file.file_stem().is_some_and(|s| *s == *m.name)
        && is_service_name(&m.name)
        && in_dir(&m.config, &own_dir)
        && m.env_file
            .as_deref()
            .is_none_or(|e| in_dir(e, &p.config_dir()))
        && (m.log_file.as_os_str().is_empty() || m.log_file == p.log_file(m.short()))
        && (binary_trusted(m, p) || m.binary == p.binary_path())
}

pub(crate) fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("damaged: {e}"))?;
    if trustworthy(&m, path) {
        Ok(m)
    } else {
        Err("it points outside burrow's own folders, so it was not written by burrow".into())
    }
}

pub fn load_manifest(name: &str) -> Result<Manifest, Failure> {
    let path = manifest_path(name);
    if !path.exists() {
        return Err(
            Failure::new(format!("no burrow service named {name}")).hint(format!(
                "see the installed ones with: {}",
                crate::base::failure::cmd("service list")
            )),
        );
    }
    read_manifest(&path).map_err(|why| {
        Failure::new(format!("ignoring {}: {why}", path.display())).hint(format!(
            "delete it as an administrator if you did not create it: {}",
            path.display()
        ))
    })
}

pub fn any_installed() -> bool {
    std::fs::read_dir(Platform::current().state_dir()).is_ok_and(|dir| {
        dir.flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "json"))
    })
}

pub fn services_hidden() -> bool {
    let dir = Platform::current().state_dir();
    dir.exists() && std::fs::read_dir(&dir).is_err()
}

pub fn all_manifests() -> Vec<Manifest> {
    let Ok(dir) = std::fs::read_dir(Platform::current().state_dir()) else {
        return Vec::new();
    };
    let mut list: Vec<Manifest> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read_manifest(&e.path()).ok())
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

pub fn writable_hint(config: &Path) -> String {
    format!(
        "install the service again so it may change its config; nothing moves: {}",
        crate::base::failure::admin(&format!(
            "service install -c {} --admin-writable",
            crate::base::failure::path_arg(config)
        ))
    )
}

static CONFIG: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn note_config(path: &Path) {
    let _ = CONFIG.set(crate::base::fs::canonical_or_same(path));
}

pub fn low_port_hint() -> String {
    low_port_hint_for(CONFIG.get().map(PathBuf::as_path))
}

fn low_port_hint_for(config: Option<&Path>) -> String {
    use crate::base::{failure, ports};
    match config.filter(|c| in_service_folder(c)) {
        Some(c) => ports::low_port_service_fix(
            ports::os(),
            &failure::admin(&format!("service install -c {}", failure::path_arg(c))),
        ),
        None => {
            let exe = std::env::current_exe()
                .map(|p| failure::arg(&p.to_string_lossy()))
                .unwrap_or_else(|_| "$(command -v burrow)".into());
            ports::low_port_fix(ports::os(), failure::command(), &exe)
        }
    }
}

pub fn in_service_folder(config: &Path) -> bool {
    config.parent().and_then(Path::parent) == Some(Platform::current().config_dir().as_path())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_low_port_hint_fits_how_burrow_runs() {
        let installed = Platform::current()
            .config_dir()
            .join("home")
            .join("server.yaml");
        let service = low_port_hint_for(Some(&installed));
        let by_hand = low_port_hint_for(Some(Path::new("/home/me/server.yaml")));
        if cfg!(target_os = "linux") {
            assert!(
                service.contains("install it again") && !service.contains("setcap"),
                "{service}"
            );
            assert!(
                by_hand.contains("setcap") && by_hand.contains("service install grants it"),
                "{by_hand}"
            );
        }
        assert_eq!(by_hand, low_port_hint_for(None));
    }

    use super::*;

    #[test]
    fn service_names_carry_the_burrow_prefix_once() {
        assert_eq!(service_name("home"), "burrow-home");
        assert_eq!(service_name("burrow-home"), "burrow-home");
        assert_eq!(service_name("burrow"), "burrow");
        assert_eq!(short_name("burrow-home"), "home");
        assert_eq!(short_name("home"), "home");
        assert!(is_service_name("burrow") && is_service_name("burrow-x"));
        assert!(!is_service_name("burrowed") && !is_service_name("nginx"));
    }
}
