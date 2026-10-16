use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
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
    let (file, writable) = open_shared(&path).map_err(|e| open_failure(&path, &e))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let holder = std::fs::read_to_string(path.with_extension("info")).unwrap_or_default();
            let holder = holder.trim();
            let who = if holder.is_empty() {
                String::new()
            } else {
                format!(" ({holder})")
            };
            return Err(
                Failure::new(format!("{} is already running{who}", identity.describe())).hint(
                    if cfg!(windows) {
                        "stop the other instance first; if it runs as a service: burrow service stop (as administrator)"
                    } else {
                        "stop the other instance first; if it runs as a service: sudo burrow service stop"
                    },
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
        write_note(&path.with_extension("info"), &note);
    }
    Ok(InstanceLock { _file: file, path })
}

fn no_follow(o: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    o
}

fn single_file(f: File) -> std::io::Result<File> {
    let meta = f.metadata()?;
    #[cfg(unix)]
    let linked = std::os::unix::fs::MetadataExt::nlink(&meta) > 1;
    #[cfg(not(unix))]
    let linked = false;
    if !meta.is_file() || linked {
        return Err(std::io::Error::other("not a plain file; remove it"));
    }
    Ok(f)
}

fn open_shared(path: &Path) -> std::io::Result<(File, bool)> {
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(f) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                f.set_permissions(std::fs::Permissions::from_mode(0o666))?;
            }
            return Ok((f, true));
        }
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        Err(_) => {}
    }
    match no_follow(OpenOptions::new().read(true).write(true)).open(path) {
        Ok(f) => Ok((single_file(f)?, true)),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok((
            single_file(no_follow(OpenOptions::new().read(true)).open(path)?)?,
            false,
        )),
        Err(e) => Err(e),
    }
}

fn write_note(path: &Path, note: &str) {
    let Ok((mut f, true)) = open_shared(path) else {
        return;
    };
    let _ = f.set_len(0);
    let _ = f.write_all(note.as_bytes());
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

    #[cfg(unix)]
    #[test]
    fn planted_links_are_not_followed_or_changed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("server.yaml");
        std::fs::write(&cfg, "x").unwrap();
        let id = Identity::server(&cfg);
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o600)).unwrap();
        let lock = dir.path().join(format!("{}.lock", id.key()));

        std::os::unix::fs::symlink(&victim, &lock).unwrap();
        assert!(acquire_in(dir.path(), &id, &cfg).is_err());
        std::fs::remove_file(&lock).unwrap();

        std::fs::hard_link(&victim, &lock).unwrap();
        assert!(acquire_in(dir.path(), &id, &cfg).is_err());
        std::fs::remove_file(&lock).unwrap();

        std::os::unix::fs::symlink(&victim, lock.with_extension("info")).unwrap();
        let held = acquire_in(dir.path(), &id, &cfg).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert_eq!(
            std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(held);
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
