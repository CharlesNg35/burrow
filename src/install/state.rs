use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use blake2::{Blake2s256, Digest};

use super::{Manifest, Platform, all_manifests};

static CHOSEN: OnceLock<PathBuf> = OnceLock::new();

pub fn choose_state_dir(dir: PathBuf) {
    let _ = CHOSEN.set(dir);
}

pub fn service_state_dir(short: &str) -> PathBuf {
    let p = Platform::current();
    match p {
        Platform::Windows => p.state_dir().join(short),
        _ => p
            .state_dir()
            .parent()
            .map_or_else(|| PathBuf::from("/var/lib/burrow"), Path::to_path_buf)
            .join(short),
    }
}

pub fn state_dir_for(config: &Path) -> PathBuf {
    if let Some(d) = CHOSEN.get() {
        return d.clone();
    }
    let config = crate::base::fs::canonical_or_same(config);
    if let Some(m) = installed_for(&config, &all_manifests()) {
        return m
            .web_state
            .clone()
            .unwrap_or_else(|| service_state_dir(m.short()));
    }
    own_state_dir(&config, dirs::state_dir().or_else(dirs::data_local_dir))
}

pub fn missing_state_folder(config: &Path) -> Option<String> {
    missing_in(config, &all_manifests())
}

fn missing_in(config: &Path, manifests: &[Manifest]) -> Option<String> {
    let config = crate::base::fs::canonical_or_same(config);
    let m = installed_for(&config, manifests)?;
    m.web_state.is_none().then(|| {
        format!(
            "{} was installed without its state folder, which uploads and the local CA need; install it again: {}",
            m.name,
            crate::base::failure::admin(&format!(
                "service install -c {}",
                crate::base::failure::path_arg(&m.source_config)
            ))
        )
    })
}

fn installed_for<'a>(config: &Path, manifests: &'a [Manifest]) -> Option<&'a Manifest> {
    manifests.iter().find(|m| {
        m.role == super::Role::Server
            && [&m.config, &m.source_config]
                .iter()
                .any(|c| crate::base::fs::canonical_or_same(c) == config)
    })
}

fn own_state_dir(config: &Path, base: Option<PathBuf>) -> PathBuf {
    let digest = Blake2s256::new()
        .chain_update(config.to_string_lossy().as_bytes())
        .finalize();
    let stem = config
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("server");
    let folder = format!("{stem}-{}", hex::encode(&digest[..4]));
    base.unwrap_or_else(|| config.parent().unwrap_or(Path::new(".")).to_path_buf())
        .join("burrow")
        .join("servers")
        .join(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_installed_without_its_state_folder_is_told_to_install_again() {
        let mut m = manifest(
            "burrow-home",
            "server",
            "/etc/burrow/home/server.yaml",
            "/srv/home.yaml",
        );
        let why = missing_in(
            Path::new("/etc/burrow/home/server.yaml"),
            std::slice::from_ref(&m),
        )
        .unwrap();
        assert!(why.starts_with("burrow-home was installed without its state folder, which uploads and the local CA need; install it again: "), "{why}");
        assert!(why.contains("service install -c /srv/home.yaml"), "{why}");
        m.web_state = Some("/var/lib/burrow/home".into());
        assert_eq!(
            missing_in(Path::new("/etc/burrow/home/server.yaml"), &[m]),
            None
        );
        assert_eq!(missing_in(Path::new("/srv/other.yaml"), &[]), None);
    }

    fn manifest(name: &str, role: &str, config: &str, source: &str) -> Manifest {
        Manifest {
            name: name.into(),
            role: crate::install::Role::parse(role).unwrap(),
            config: config.into(),
            source_config: source.into(),
            env_file: None,
            log_file: PathBuf::new(),
            binary: PathBuf::new(),
            copied_binary: false,
            account: "burrow-home".into(),
            created_account: false,
            low_ports: false,
            files: Vec::new(),
            version: String::new(),
            admin_writable: false,
            web_state: None,
        }
    }

    #[test]
    fn a_server_run_by_hand_keeps_its_state_in_the_users_state_folder_per_config() {
        let base = Some(PathBuf::from("/home/u/.local/state"));
        let a = own_state_dir(Path::new("/etc/burrow/server.yaml"), base.clone());
        let b = own_state_dir(Path::new("/srv/other/server.yaml"), base);
        assert!(
            a.starts_with("/home/u/.local/state/burrow/servers"),
            "{a:?}"
        );
        assert!(
            a.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("server-")
        );
        assert_ne!(a, b);
    }

    #[test]
    fn an_installed_server_config_uses_its_services_folder() {
        let list = vec![
            manifest(
                "burrow-lap",
                "visitor",
                "/etc/burrow/home/server.yaml",
                "/x",
            ),
            manifest(
                "burrow-home",
                "server",
                "/etc/burrow/home/server.yaml",
                "/root/server.yaml",
            ),
        ];
        let found = installed_for(Path::new("/root/server.yaml"), &list).unwrap();
        assert_eq!(found.short(), "home");
        assert!(installed_for(Path::new("/elsewhere.yaml"), &list).is_none());
        assert!(service_state_dir("home").ends_with("burrow/home") || cfg!(windows));
    }
}
