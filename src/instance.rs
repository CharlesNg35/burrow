use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use blake2::{Blake2s256, Digest};

use crate::failure::Failure;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    Server { config: PathBuf },
    Client { server: String, name: String },
}

impl Identity {
    pub fn server(config: &Path) -> Identity {
        Identity::Server {
            config: std::fs::canonicalize(config).unwrap_or_else(|_| config.to_path_buf()),
        }
    }

    pub fn client(server: &str, name: &str) -> Identity {
        Identity::Client {
            server: server.trim().to_ascii_lowercase(),
            name: name.to_string(),
        }
    }

    fn key(&self) -> String {
        let text = match self {
            Identity::Server { config } => format!("server\0{}", config.display()),
            Identity::Client { server, name } => format!("client\0{server}\0{name}"),
        };
        let digest = Blake2s256::new().chain_update(text.as_bytes()).finalize();
        digest[..8].iter().map(|b| format!("{b:02x}")).collect()
    }

    fn describe(&self) -> String {
        match self {
            Identity::Server { config } => format!("a burrow server for {}", config.display()),
            Identity::Client { server, name } => format!("a burrow client \"{name}\" for {server}"),
        }
    }
}

pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("BURROW_INSTANCE_DIR") {
        return PathBuf::from(dir);
    }
    let service_dir = if cfg!(target_os = "linux") {
        Some(PathBuf::from("/run/burrow-instances"))
    } else if cfg!(target_os = "macos") {
        Some(PathBuf::from(
            "/Library/Application Support/burrow/instances",
        ))
    } else {
        None
    };
    if let Some(dir) = service_dir.filter(|d| d.is_dir()) {
        return dir;
    }
    if cfg!(windows) {
        std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("burrow")
            .join("instances")
    } else {
        PathBuf::from("/tmp/burrow-instances")
    }
}

fn prepare_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o1777))?;
    }
    Ok(())
}

pub fn acquire(identity: &Identity, config: &Path) -> Result<InstanceLock, Failure> {
    acquire_in(&default_dir(), identity, config)
}

pub fn acquire_in(dir: &Path, identity: &Identity, config: &Path) -> Result<InstanceLock, Failure> {
    prepare_dir(dir).map_err(|e| {
        Failure::new(format!(
            "cannot create {}: {}",
            dir.display(),
            crate::failure::file_reason(&e)
        ))
        .hint("burrow keeps a small lock file there so the same tunnel cannot run twice")
    })?;
    let path = dir.join(format!("{}.lock", identity.key()));
    let opened = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path);
    let (mut file, writable) = match opened {
        Ok(f) => (f, true),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => match File::open(&path) {
            Ok(f) => (f, false),
            Err(e) => return Err(open_failure(&path, &e)),
        },
        Err(e) => return Err(open_failure(&path, &e)),
    };
    #[cfg(unix)]
    if writable {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
    }
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let mut holder = String::new();
            let _ = file.read_to_string(&mut holder);
            let holder = holder.trim();
            let who = if holder.is_empty() {
                String::new()
            } else {
                format!(" ({holder})")
            };
            return Err(
                Failure::new(format!("{} is already running{who}", identity.describe())).hint(
                    "stop the other instance first; if it runs as a service: burrow service stop",
                ),
            );
        }
        Err(TryLockError::Error(e)) => {
            return Err(Failure::new(format!(
                "cannot lock {}: {}",
                path.display(),
                crate::failure::file_reason(&e)
            )));
        }
    }
    let note = format!(
        "pid {}, config {}",
        std::process::id(),
        std::fs::canonicalize(config)
            .unwrap_or_else(|_| config.to_path_buf())
            .display()
    );
    if writable {
        let _ = file.set_len(0);
        let _ = file.seek(SeekFrom::Start(0));
        let _ = file.write_all(note.as_bytes());
        let _ = file.flush();
    }
    Ok(InstanceLock { _file: file, path })
}

fn open_failure(path: &Path, e: &std::io::Error) -> Failure {
    Failure::new(format!("cannot open {}: {}", path.display(), crate::failure::file_reason(e)))
        .hint("check the permissions of that file; it only holds a lock and can be deleted when burrow is not running")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_of_the_same_identity_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("server.yaml");
        std::fs::write(&cfg, "x").unwrap();
        let id = Identity::server(&cfg);
        let first = acquire_in(dir.path(), &id, &cfg).unwrap();
        let err = acquire_in(dir.path(), &id, &cfg).err().unwrap();
        assert!(
            err.message.contains("is already running"),
            "{}",
            err.message
        );
        assert!(
            err.message.contains(&format!("pid {}", std::process::id())),
            "{}",
            err.message
        );
        drop(first);
        assert!(
            acquire_in(dir.path(), &id, &cfg).is_ok(),
            "lock is released when the holder ends"
        );
    }

    #[test]
    fn different_identities_run_side_by_side() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("c.yaml");
        std::fs::write(&cfg, "x").unwrap();
        let a = acquire_in(
            dir.path(),
            &Identity::client("tunnel.example.com:7835", "home"),
            &cfg,
        )
        .unwrap();
        let b = acquire_in(
            dir.path(),
            &Identity::client("tunnel.example.com:7835", "office"),
            &cfg,
        )
        .unwrap();
        let c = acquire_in(
            dir.path(),
            &Identity::client("other.example.com:7835", "home"),
            &cfg,
        )
        .unwrap();
        let same = acquire_in(
            dir.path(),
            &Identity::client("TUNNEL.example.com:7835", "home"),
            &cfg,
        );
        assert!(same.is_err(), "host names compare case-insensitively");
        drop((a, b, c));
    }

    #[test]
    fn server_identity_follows_the_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("server.yaml");
        std::fs::write(&cfg, "x").unwrap();
        let direct = Identity::server(&cfg);
        let via_dot = Identity::server(&dir.path().join(".").join("server.yaml"));
        assert_eq!(direct, via_dot);
    }
}
