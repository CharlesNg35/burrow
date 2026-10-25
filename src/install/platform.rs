use std::path::{Path, PathBuf};

use super::trust::trusted_folder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "linux") {
            Platform::Linux
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Other
        }
    }

    fn program_data() -> PathBuf {
        std::env::var_os("ProgramData")
            .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from)
    }

    pub fn config_dir(self) -> PathBuf {
        match self {
            Platform::Windows => Self::program_data().join("burrow"),
            _ => PathBuf::from("/etc/burrow"),
        }
    }

    pub fn state_dir(self) -> PathBuf {
        match self {
            Platform::Windows => Self::program_data().join("burrow").join("services"),
            Platform::MacOs => PathBuf::from("/Library/Application Support/burrow/services"),
            _ => PathBuf::from("/var/lib/burrow/services"),
        }
    }

    pub fn jobs_dir(self) -> PathBuf {
        match self {
            Platform::Windows => Self::program_data().join("burrow").join("jobs"),
            _ => self
                .state_dir()
                .parent()
                .map_or_else(|| PathBuf::from("/var/lib/burrow"), Path::to_path_buf)
                .join("jobs"),
        }
    }

    pub fn instances_dir(self) -> Option<PathBuf> {
        match self {
            Platform::Linux => Some(PathBuf::from("/run/burrow-instances")),
            Platform::MacOs => self.state_dir().parent().map(|base| base.join("instances")),
            _ => None,
        }
    }

    pub fn launchd_folders(self) -> Vec<(PathBuf, u32)> {
        let state = self.state_dir();
        let base = state.parent().map(Path::to_path_buf).unwrap_or_default();
        vec![
            (self.config_dir(), 0o755),
            (base.clone(), 0o755),
            (state, 0o755),
            (
                self.instances_dir()
                    .unwrap_or_else(|| base.join("instances")),
                0o1777,
            ),
            (self.log_dir(), 0o755),
        ]
    }

    pub fn log_dir(self) -> PathBuf {
        match self {
            Platform::Windows => Self::program_data().join("burrow").join("logs"),
            _ => PathBuf::from("/var/log/burrow"),
        }
    }

    pub fn log_file(self, short: &str) -> PathBuf {
        match self {
            Platform::Windows => self.log_dir().join(format!("{short}.log")),
            _ => self.log_dir().join(short).join(format!("{short}.log")),
        }
    }

    pub fn binary_path(self) -> PathBuf {
        match self {
            Platform::Windows => std::env::var_os("ProgramFiles")
                .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from)
                .join("burrow")
                .join("burrow.exe"),
            _ => PathBuf::from("/usr/local/bin/burrow"),
        }
    }

    pub fn service_binary(self) -> PathBuf {
        let usual = self.binary_path();
        match usual.parent() {
            Some(dir) if !trusted_folder(dir) => {
                self.state_dir().with_file_name("bin").join("burrow")
            }
            _ => usual,
        }
    }

    pub fn default_account(self, service: &str) -> String {
        match self {
            Platform::Windows => format!(r"NT SERVICE\{service}"),
            Platform::MacOs => "_burrow".into(),
            _ => "burrow".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_instance_folder_is_named_in_one_place() {
        assert_eq!(
            Platform::Linux.instances_dir(),
            Some(PathBuf::from("/run/burrow-instances"))
        );
        assert_eq!(
            Platform::MacOs.instances_dir(),
            Some(PathBuf::from(
                "/Library/Application Support/burrow/instances"
            ))
        );
        assert_eq!(Platform::Windows.instances_dir(), None);
        assert!(Platform::MacOs.launchd_folders().contains(&(
            PathBuf::from("/Library/Application Support/burrow/instances"),
            0o1777
        )));
    }
}
