mod lock;
mod note;
pub mod publish;

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use blake2::{Blake2s256, Digest};

use crate::base::failure::Failure;
use lock::held_by_a_trusted_instance;
#[cfg(windows)]
pub use lock::running_as_service;
pub use lock::{acquire, acquire_in, held, held_in};
#[cfg(unix)]
pub(crate) use note::alive;
pub use note::{Running, running, running_in};
use note::{no_follow, single_file};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    Server { config: PathBuf },
    Client { server: String, name: String },
}

impl Identity {
    pub fn server(config: &Path) -> Identity {
        Identity::Server {
            config: crate::base::fs::canonical_or_same(config),
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
        hex::encode(&digest[..8])
    }

    fn describe(&self) -> String {
        match self {
            Identity::Server { config } => format!("a burrow server for {}", config.display()),
            Identity::Client { server, name } => format!("a burrow client \"{name}\" for {server}"),
        }
    }
}

pub struct InstanceLock {
    _file: Option<File>,
    path: PathBuf,
    noted: bool,
    _fresh: Option<Fresh>,
}

const REFRESH: std::time::Duration = std::time::Duration::from_secs(3600);

pub struct Fresh {
    _stop: std::sync::mpsc::Sender<()>,
}

pub fn keep_fresh(paths: Vec<PathBuf>) -> Fresh {
    keep_fresh_every(paths, REFRESH)
}

fn keep_fresh_every(paths: Vec<PathBuf>, every: std::time::Duration) -> Fresh {
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let _ = std::thread::Builder::new()
        .name("burrow-fresh".into())
        .spawn(move || {
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(every)
            {
                for path in &paths {
                    touch(path);
                }
            }
        });
    Fresh { _stop: stop }
}

fn touch(path: &Path) {
    if let Ok(f) = no_follow(OpenOptions::new().read(true).write(true)).open(path) {
        let now = std::time::SystemTime::now();
        let _ = f.set_times(
            std::fs::FileTimes::new()
                .set_accessed(now)
                .set_modified(now),
        );
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if self.noted {
            let _ = std::fs::remove_file(self.path.with_extension(NOTE_EXTENSION));
        }
    }
}

impl InstanceLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub const LOCK_EXTENSION: &str = "lock";
pub const NOTE_EXTENSION: &str = "info";
pub const STATUS_EXTENSION: &str = "status";

pub fn service_dir() -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        crate::install::Platform::Linux.instances_dir()
    } else if cfg!(target_os = "macos") {
        crate::install::Platform::MacOs.instances_dir()
    } else {
        None
    }
}

fn left_behind(draft: &Path) -> bool {
    std::fs::symlink_metadata(draft)
        .and_then(|m| m.modified())
        .is_ok_and(|t| {
            t.elapsed()
                .is_ok_and(|age| age > std::time::Duration::from_secs(60))
        })
}

pub fn remove_unused(dir: &Path) {
    if !safe_dir(dir) {
        return;
    }
    let Ok(listing) = std::fs::read_dir(dir) else {
        return;
    };
    for path in listing.flatten().map(|e| e.path()) {
        match path.extension().and_then(|x| x.to_str()) {
            Some(LOCK_EXTENSION) => {
                let mut opts = OpenOptions::new();
                opts.read(true).write(true);
                let held = no_follow(&mut opts)
                    .open(&path)
                    .and_then(single_file)
                    .ok()
                    .filter(|f| f.try_lock().is_ok());
                if let Some(lock) = held {
                    let _ = std::fs::remove_file(path.with_extension(NOTE_EXTENSION));
                    let _ = std::fs::remove_file(&path);
                    drop(lock);
                }
            }
            Some(STATUS_EXTENSION) if stale_status(&path) => {
                let _ = std::fs::remove_file(&path);
            }
            Some(crate::base::fs::TEMP_EXTENSION) if left_behind(&path) => {
                let _ = std::fs::remove_file(&path);
            }
            _ => {}
        }
    }
    let _ = std::fs::remove_dir(dir);
}

#[cfg(unix)]
fn stale_status(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<u32>().ok())
        .is_some_and(|pid| !alive(pid))
}

#[cfg(not(unix))]
fn stale_status(_path: &Path) -> bool {
    false
}

pub fn dirs_with(dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![dir.to_path_buf()];
    if std::env::var_os("BURROW_INSTANCE_DIR").is_none() && dir == default_dir() {
        dirs.extend(
            [service_dir(), Some(fallback_dir())]
                .into_iter()
                .flatten()
                .filter(|d| d.is_dir() && d != dir),
        );
    }
    dirs
}

fn fallback_dir() -> PathBuf {
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

pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("BURROW_INSTANCE_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = service_dir().filter(|d| d.is_dir()) {
        return dir;
    }
    fallback_dir()
}

pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !std::fs::symlink_metadata(dir)?.is_dir() {
            return Err(std::io::Error::other(format!(
                "{} is not a folder",
                dir.display()
            )));
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        let icacls = |args: &[&str]| {
            std::process::Command::new("icacls")
                .arg(dir)
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success())
        };
        let done = icacls(&["/setowner", "*S-1-5-32-544"])
            && icacls(&["/reset"])
            && icacls(&[
                "/inheritance:r",
                "/grant:r",
                "*S-1-5-18:(OI)(CI)F",
                "*S-1-5-32-544:(OI)(CI)F",
            ]);
        if !done {
            return Err(std::io::Error::other(format!(
                "cannot restrict {} to administrators",
                dir.display()
            )));
        }
    }
    Ok(())
}

pub fn prepare(dir: &Path) -> std::io::Result<()> {
    prepare_dir(dir)?;
    if !safe_dir(dir) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "it is a link or folder another user controls",
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub fn safe_dir(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let me = rustix::process::geteuid().as_raw();
    let trusted = |uid: u32| uid == 0 || uid == me;
    let Ok(link) = std::fs::symlink_metadata(dir) else {
        return true;
    };
    if link.file_type().is_symlink() && !trusted(link.uid()) {
        return false;
    }
    let Ok(target) = std::fs::metadata(dir) else {
        return false;
    };
    let sticky = target.mode() & 0o1000 != 0;
    let open_to_all = target.mode() & 0o002 != 0;
    let owned = trusted(target.uid());
    target.is_dir()
        && if sticky {
            owned || me != 0
        } else {
            owned && !open_to_all
        }
}

#[cfg(not(unix))]
pub fn safe_dir(_dir: &Path) -> bool {
    true
}

fn prepare_dir(dir: &Path) -> std::io::Result<()> {
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags};
        let made = rustix::fs::open(
            dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        rustix::fs::fchmod(&made, Mode::from_bits_truncate(0o1777))?;
    }
    Ok(())
}

fn open_failure(path: &Path, e: &std::io::Error) -> Failure {
    Failure::new(format!("cannot open {}: {}", path.display(), crate::base::failure::file_reason(e)))
        .hint("check the permissions of that file; it only holds a lock and can be deleted when burrow is not running")
}

#[cfg(test)]
mod tests;
