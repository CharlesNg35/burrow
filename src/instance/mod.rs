pub mod publish;

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use blake2::{Blake2s256, Digest};

use crate::base::failure::Failure;

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
            let _ = std::fs::remove_file(self.path.with_extension("info"));
        }
    }
}

impl InstanceLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn service_dir() -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        Some(PathBuf::from("/run/burrow-instances"))
    } else if cfg!(target_os = "macos") {
        Some(PathBuf::from(
            "/Library/Application Support/burrow/instances",
        ))
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
            Some("lock") => {
                let mut opts = OpenOptions::new();
                opts.read(true).write(true);
                let held = no_follow(&mut opts)
                    .open(&path)
                    .and_then(single_file)
                    .ok()
                    .filter(|f| f.try_lock().is_ok());
                if let Some(lock) = held {
                    let _ = std::fs::remove_file(path.with_extension("info"));
                    let _ = std::fs::remove_file(&path);
                    drop(lock);
                }
            }
            Some("status") if stale_status(&path) => {
                let _ = std::fs::remove_file(&path);
            }
            Some("new") if left_behind(&path) => {
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

pub fn acquire(identity: &Identity, config: &Path) -> Result<InstanceLock, Failure> {
    let dir = default_dir();
    let elsewhere = dirs_with(&dir)
        .iter()
        .skip(1)
        .find_map(|other| held_elsewhere(other, identity));
    let taken = match elsewhere {
        Some(f) => Err(f),
        None => acquire_in(&dir, identity, config),
    };
    #[cfg(windows)]
    if let Err(f) = &taken
        && AS_SERVICE.load(std::sync::atomic::Ordering::Relaxed)
    {
        crate::say_errln!("warning: {}; a service starts anyway", f.message);
        return Ok(InstanceLock {
            _file: None,
            path: dir.join(format!("{}.lock", identity.key())),
            noted: false,
            _fresh: None,
        });
    }
    taken
}

#[cfg(windows)]
static AS_SERVICE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
pub fn running_as_service() {
    AS_SERVICE.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn acquire_in(dir: &Path, identity: &Identity, config: &Path) -> Result<InstanceLock, Failure> {
    let path = dir.join(format!("{}.lock", identity.key()));
    let prepared = prepare_dir(dir);
    if prepared.is_ok() && !safe_dir(dir) {
        return Ok(unlocked(dir, &path));
    }
    if let Err(e) = prepared {
        if planted(dir) {
            return Ok(unlocked(dir, &path));
        }
        return Err(Failure::new(format!(
            "cannot create {}: {}",
            dir.display(),
            crate::base::failure::file_reason(&e)
        ))
        .hint("burrow keeps a small lock file there so the same tunnel cannot run twice"));
    }
    let mut attempts = 0;
    let (file, writable) = loop {
        attempts += 1;
        let opened = match open_shared(&path) {
            Err(_) if extra_links(&path) => match replace_linked(&path) {
                Ok(Some(opened)) => Ok(opened),
                Ok(None) => return Err(already_running(identity, &path)),
                Err(e) => Err(e),
            },
            other => other,
        };
        let (file, writable) = match opened {
            Ok(opened) => opened,
            Err(_) if planted(&path) => return Ok(unlocked(&path, &path)),
            Err(_) if planted(dir) => return Ok(unlocked(dir, &path)),
            Err(e) => return Err(open_failure(&path, &e)),
        };
        match file.try_lock() {
            Ok(()) if attempts < 5 && !names(&path, &file) => continue,
            Ok(()) => break (file, writable),
            Err(TryLockError::WouldBlock) if !held_by_a_trusted_instance(&path) => {
                crate::say_errln!(
                    "warning: {} is locked by a process burrow does not trust (another user's, or one that left no note); starting anyway",
                    path.display()
                );
                return Ok(InstanceLock {
                    _file: Some(file),
                    path,
                    noted: false,
                    _fresh: None,
                });
            }
            Err(TryLockError::WouldBlock) => return Err(already_running(identity, &path)),
            Err(TryLockError::Error(e)) => {
                return Err(Failure::new(format!(
                    "cannot lock {}: {}",
                    path.display(),
                    crate::base::failure::file_reason(&e)
                )));
            }
        }
    };
    let note = format!(
        "pid {}, config {}",
        std::process::id(),
        crate::base::fs::canonical_or_same(config).display()
    );
    let noted = writable && write_note(&path.with_extension("info"), &note);
    let mut fresh = vec![path.clone()];
    if noted {
        fresh.push(path.with_extension("info"));
    }
    Ok(InstanceLock {
        _file: Some(file),
        path,
        noted,
        _fresh: Some(keep_fresh(fresh)),
    })
}

fn unlocked(entry: &Path, path: &Path) -> InstanceLock {
    crate::say_errln!(
        "warning: {} belongs to another user and cannot be used as a lock; starting without it",
        entry.display()
    );
    InstanceLock {
        _file: None,
        path: path.to_path_buf(),
        noted: false,
        _fresh: None,
    }
}

#[cfg(unix)]
fn planted(entry: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let me = rustix::process::geteuid().as_raw();
    std::fs::symlink_metadata(entry).is_ok_and(|m| m.uid() != 0 && m.uid() != me)
}

#[cfg(not(unix))]
fn planted(_entry: &Path) -> bool {
    false
}

fn already_running(identity: &Identity, lock: &Path) -> Failure {
    let holder = std::fs::File::open(lock.with_extension("info"))
        .ok()
        .and_then(|f| {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::Read::take(f, 4096), &mut text).ok()?;
            Some(text)
        })
        .unwrap_or_default();
    let mut holder = holder.trim().to_string();
    if let Identity::Server { config } = identity {
        holder = holder.replace(&format!(", config {}", config.display()), "");
    }
    let who = if holder.is_empty() {
        String::new()
    } else {
        format!(" ({holder})")
    };
    let hint = if crate::base::failure::SERVICES {
        format!(
            "stop the other instance first; if it runs as a service, find it with: {}",
            crate::base::failure::cmd("service list")
        )
    } else {
        "stop the other instance first".to_string()
    };
    Failure::new(format!("{} is already running{who}", identity.describe())).hint(hint)
}

pub fn held(identity: &Identity) -> Option<Failure> {
    held_in(&dirs_with(&default_dir()), identity)
}

pub fn held_in(dirs: &[PathBuf], identity: &Identity) -> Option<Failure> {
    dirs.iter().find_map(|dir| held_elsewhere(dir, identity))
}

fn held_elsewhere(dir: &Path, identity: &Identity) -> Option<Failure> {
    let path = dir.join(format!("{}.lock", identity.key()));
    let file = no_follow(OpenOptions::new().read(true))
        .open(&path)
        .and_then(single_file)
        .ok()?;
    let busy = matches!(file.try_lock(), Err(TryLockError::WouldBlock));
    (busy && held_by_a_trusted_instance(&path)).then(|| already_running(identity, &path))
}

fn held_by_a_trusted_instance(lock: &Path) -> bool {
    let deadline = std::time::Instant::now() + NOTE_PATIENCE;
    loop {
        if trusted_note(&lock.with_extension("info")).is_some() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

const NOTE_PATIENCE: std::time::Duration = std::time::Duration::from_secs(1);

fn no_follow(o: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    o
}

#[cfg(unix)]
fn extra_links(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.nlink() > 1)
}

#[cfg(not(unix))]
fn extra_links(_path: &Path) -> bool {
    false
}

fn replace_linked(path: &Path) -> std::io::Result<Option<(File, bool)>> {
    let old = no_follow(OpenOptions::new().read(true)).open(path)?;
    match old.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) if held_by_a_trusted_instance(path) => return Ok(None),
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Error(e)) => return Err(e),
    }
    if names(path, &old) {
        std::fs::remove_file(path)?;
    }
    let opened = open_shared(path).map(Some);
    drop(old);
    opened
}

#[cfg(unix)]
fn names(path: &Path, file: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(path), file.metadata()) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn names(_path: &Path, _file: &File) -> bool {
    true
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

fn write_note(path: &Path, note: &str) -> bool {
    let Ok((mut f, true)) = open_shared(path) else {
        return false;
    };
    let written = f.set_len(0).is_ok() && f.write_all(note.as_bytes()).is_ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o644));
    }
    written
}

pub struct Running {
    pub pid: u32,
    pub config: PathBuf,
}

pub fn running() -> Vec<Running> {
    dirs_with(&default_dir())
        .iter()
        .flat_map(|d| running_in(d))
        .collect()
}

pub fn running_in(dir: &Path) -> Vec<Running> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Running> = listing
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "info"))
        .filter_map(|p| trusted_note(&p))
        .collect();
    found.sort_by(|a, b| a.config.cmp(&b.config));
    found
}

fn parse_note(text: &str) -> Option<Running> {
    let (pid, config) = text.trim().strip_prefix("pid ")?.split_once(", config ")?;
    Some(Running {
        pid: pid.parse().ok()?,
        config: PathBuf::from(config),
    })
}

#[cfg(unix)]
fn trusted_note(path: &Path) -> Option<Running> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let m = file.metadata().ok()?;
    if !m.is_file() || m.len() > 4096 || m.mode() & 0o022 != 0 {
        return None;
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::Read::take(&mut file, 4097), &mut text).ok()?;
    if text.len() > 4096 {
        return None;
    }
    let note = parse_note(&text)?;
    (trusted_owner(m.uid(), &note.config) && runs_as(note.pid, m.uid())).then_some(note)
}

#[cfg(unix)]
fn trusted_owner(uid: u32, config: &Path) -> bool {
    let me = rustix::process::geteuid().as_raw();
    uid == 0
        || uid == me
        || crate::install::all_manifests().iter().any(|m| {
            (m.created_account || crate::base::fs::canonical_or_same(&m.config) == config)
                && crate::base::process::account_ids(&m.account).is_ok_and(|(id, _)| id == uid)
        })
}

#[cfg(not(unix))]
fn trusted_note(path: &Path) -> Option<Running> {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() <= 4096) {
        return None;
    }
    let mut text = String::new();
    std::io::Read::read_to_string(
        &mut std::io::Read::take(std::fs::File::open(path).ok()?, 4097),
        &mut text,
    )
    .ok()?;
    if text.len() > 4096 {
        return None;
    }
    let note = parse_note(&text)?;
    runs_as(note.pid, 0).then_some(note)
}

#[cfg(target_os = "linux")]
fn pid_owner(pid: u32) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(format!("/proc/{pid}"))
        .ok()
        .map(|m| m.uid())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn pid_owner(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-o", "uid=", "-p", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(unix)]
fn runs_as(pid: u32, owner: u32) -> bool {
    pid_owner(pid) == Some(owner)
}

#[cfg(windows)]
fn runs_as(pid: u32, _owner: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| {
            let text = String::from_utf8_lossy(&o.stdout).to_ascii_lowercase();
            text.starts_with("\"burrow") && text.contains(&format!("\"{pid}\""))
        })
}

#[cfg(not(any(unix, windows)))]
fn runs_as(_pid: u32, _owner: u32) -> bool {
    false
}

#[cfg(unix)]
pub(crate) fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    rustix::process::Pid::from_raw(pid).is_some_and(|pid| {
        matches!(
            rustix::process::test_kill_process(pid),
            Ok(()) | Err(rustix::io::Errno::PERM)
        )
    })
}

fn open_failure(path: &Path, e: &std::io::Error) -> Failure {
    Failure::new(format!("cannot open {}: {}", path.display(), crate::base::failure::file_reason(e)))
        .hint("check the permissions of that file; it only holds a lock and can be deleted when burrow is not running")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_of_a_running_instance_stay_fresh_for_tmp_cleaners() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.status");
        std::fs::write(&file, "x").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        let age = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
        let set_old = || {
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_accessed(old)
                        .set_modified(old),
                )
                .unwrap()
        };
        set_old();
        let fresh = keep_fresh_every(vec![file.clone()], std::time::Duration::from_millis(20));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while age(&file) == old && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_ne!(age(&file), old);
        drop(fresh);
        std::thread::sleep(std::time::Duration::from_millis(50));
        set_old();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(age(&file), old, "a dropped guard kept touching");
        std::fs::remove_file(&file).unwrap();
        let fresh = keep_fresh_every(vec![file.clone()], std::time::Duration::from_millis(20));
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(fresh);
        assert!(!file.exists(), "touching must not create a removed file");
    }

    #[cfg(unix)]
    fn soon(mut done: impl FnMut() -> bool) -> bool {
        (0..100).any(|_| {
            done() || {
                std::thread::sleep(std::time::Duration::from_millis(10));
                false
            }
        })
    }

    #[cfg(unix)]
    #[test]
    fn the_install_check_only_reads_the_lock_folders() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("server.yaml");
        let id = Identity::server(&cfg);
        let missing = dir.path().join("missing");
        let used = dir.path().join("used");
        std::fs::create_dir(&used).unwrap();
        let dirs = [missing.clone(), used.clone()];
        assert!(held_in(&dirs, &id).is_none());
        assert!(!missing.exists());
        assert_eq!(std::fs::read_dir(&used).unwrap().count(), 0);
        let lock = acquire_in(&used, &id, &cfg).unwrap();
        let f = held_in(&dirs, &id).expect("a held lock is found");
        assert!(f.message.contains("is already running"), "{}", f.message);
        drop(lock);
        assert!(held_in(&dirs, &id).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn only_locks_and_status_files_nobody_uses_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("server.yaml");
        let held = acquire_in(dir.path(), &Identity::server(&cfg), &cfg).unwrap();
        let unused =
            acquire_in(dir.path(), &Identity::client("example.com", "home"), &cfg).unwrap();
        let unused_lock = unused.path().to_path_buf();
        drop(unused);
        let stale = dir.path().join("999999999.status");
        let live = dir.path().join(format!("{}.status", std::process::id()));
        std::fs::write(&stale, "{}").unwrap();
        std::fs::write(&live, "{}").unwrap();
        assert!(soon(|| {
            remove_unused(dir.path());
            !unused_lock.exists() && !unused_lock.with_extension("info").exists()
        }));
        assert!(held.path().exists() && held.path().with_extension("info").exists());
        assert!(!stale.exists() && live.exists());
        std::fs::remove_file(&live).unwrap();
        let held_path = held.path().to_path_buf();
        drop(held);
        assert!(soon(|| {
            remove_unused(dir.path());
            !held_path.exists()
        }));
        assert!(!dir.path().exists(), "an empty folder is removed");
    }

    #[cfg(unix)]
    #[test]
    fn a_second_start_waits_for_the_note_of_one_that_just_took_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("client.yaml");
        let id = Identity::client("example.com", "home");
        let lock = dir.path().join(format!("{}.lock", id.key()));
        let (held, _) = open_shared(&lock).unwrap();
        held.try_lock().unwrap();
        let info = lock.with_extension("info");
        let note = format!(
            "pid {}, config {}",
            std::process::id(),
            crate::base::fs::canonical_or_same(&cfg).display()
        );
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert!(write_note(&info, &note));
        });
        let second = acquire_in(dir.path(), &id, &cfg).err().unwrap();
        writer.join().unwrap();
        assert!(
            second.message.contains("already running"),
            "{}",
            second.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn instances_racing_over_a_linked_lock_still_start_only_once() {
        for _ in 0..40 {
            let dir = tempfile::tempdir().unwrap();
            let cfg = dir.path().join("client.yaml");
            let id = Identity::client("example.com", "home");
            drop(acquire_in(dir.path(), &id, &cfg).unwrap());
            let lock = dir.path().join(format!("{}.lock", id.key()));
            std::fs::hard_link(&lock, dir.path().join("planted")).unwrap();
            let start = std::sync::Arc::new(std::sync::Barrier::new(4));
            let racers: Vec<_> = (0..4)
                .map(|_| {
                    let (dir, cfg, id, start) = (
                        dir.path().to_path_buf(),
                        cfg.clone(),
                        id.clone(),
                        start.clone(),
                    );
                    std::thread::spawn(move || {
                        start.wait();
                        acquire_in(&dir, &id, &cfg).ok()
                    })
                })
                .collect();
            let held: Vec<_> = racers
                .into_iter()
                .filter_map(|r| r.join().unwrap())
                .collect();
            assert_eq!(held.len(), 1, "{} instances started", held.len());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_link_to_the_lock_cannot_keep_an_instance_from_starting() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("server.yaml");
        let id = Identity::server(&cfg);
        let held = acquire_in(dir.path(), &id, &cfg).unwrap();
        let lock = held.path().to_path_buf();
        std::fs::hard_link(&lock, dir.path().join("planted")).unwrap();
        let again = acquire_in(dir.path(), &id, &cfg).err().unwrap();
        assert!(
            again.message.contains("already running"),
            "{}",
            again.message
        );
        drop(held);
        let restarted = acquire_in(dir.path(), &id, &cfg).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(restarted.path()).unwrap().nlink(), 1);
        let planted = std::fs::File::open(dir.path().join("planted")).unwrap();
        assert!(soon(|| planted.try_lock().is_ok()));
        drop(restarted);
        let ignored = acquire_in(dir.path(), &id, &cfg);
        assert!(
            ignored.is_ok(),
            "an outsider holding the old file must not lock it out"
        );
    }

    #[cfg(unix)]
    #[test]
    fn running_instances_are_read_from_their_notes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("client.yaml");
        let lock = acquire_in(dir.path(), &Identity::client("example.com", "home"), &cfg).unwrap();
        std::fs::write(
            dir.path().join("dead.info"),
            "pid 999999999, config /x/server.yaml",
        )
        .unwrap();
        std::fs::write(dir.path().join("odd.info"), "something else").unwrap();
        let found = running_in(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pid, std::process::id());
        assert!(found[0].config.ends_with("client.yaml"));
        drop(lock);
    }

    #[cfg(unix)]
    #[test]
    fn only_entries_of_another_user_count_as_planted() {
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join("x.lock");
        std::fs::write(&mine, "").unwrap();
        assert!(!planted(&mine));
        assert!(!planted(&dir.path().join("missing.lock")));
        assert!(!planted(Path::new("/")));
    }

    #[test]
    fn only_old_drafts_are_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("1.new");
        std::fs::write(&fresh, "x").unwrap();
        let old = dir.path().join("2.new");
        std::fs::write(&old, "x").unwrap();
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();
        remove_unused(dir.path());
        assert!(fresh.exists());
        assert!(!old.exists());
    }

    #[test]
    fn a_tunnel_locked_in_the_other_instance_folder_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("server.yaml");
        std::fs::write(&cfg, "x").unwrap();
        let id = Identity::server(&cfg);
        assert!(held_elsewhere(dir.path(), &id).is_none());
        let lock = acquire_in(dir.path(), &id, &cfg).unwrap();
        let found = held_elsewhere(dir.path(), &id).expect("the held lock is found");
        assert!(
            found.message.contains("is already running"),
            "{}",
            found.message
        );
        drop(lock);
        assert!(held_elsewhere(dir.path(), &id).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_in_place_of_a_note_is_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("x.info");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&note)
                .status()
                .unwrap()
                .success()
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(trusted_note(&note).is_none()).unwrap());
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true));
    }

    #[test]
    fn a_released_lock_takes_its_note_away() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("server.yaml");
        std::fs::write(&cfg, "x").unwrap();
        let id = Identity::server(&cfg);
        let lock = acquire_in(dir.path(), &id, &cfg).unwrap();
        let note = lock.path().with_extension("info");
        assert!(note.exists());
        drop(lock);
        assert!(!note.exists());
    }

    #[test]
    fn a_lock_held_without_a_trusted_note_does_not_stop_a_start() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("server.yaml");
        std::fs::write(&cfg, "x").unwrap();
        let id = Identity::server(&cfg);
        let first = acquire_in(dir.path(), &id, &cfg).unwrap();
        std::fs::remove_file(first.path().with_extension("info")).unwrap();
        assert!(acquire_in(dir.path(), &id, &cfg).is_ok());
        drop(first);
    }

    #[cfg(unix)]
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
            soon(|| acquire_in(dir.path(), &id, &cfg).is_ok()),
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
    fn a_folder_everyone_can_write_without_the_sticky_bit_gets_no_lock() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("instances");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let config = base.path().join("server.yaml");
        std::fs::write(&config, "").unwrap();
        let lock = acquire_in(&dir, &Identity::server(&config), &config).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        drop(lock);
        assert!(!safe_dir(&dir));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert!(safe_dir(&dir));
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
        let replaced = acquire_in(dir.path(), &id, &cfg).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), "");
        drop(replaced);
        std::fs::remove_file(&lock).unwrap();
        let _ = std::fs::remove_file(lock.with_extension("info"));

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
