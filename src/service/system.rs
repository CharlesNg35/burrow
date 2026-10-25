use std::path::{Path, PathBuf};
use std::process::Command;

use super::Backend;
use crate::base::failure::Failure;
#[cfg(unix)]
use crate::base::process::account_ids;
#[cfg(windows)]
use crate::base::process::run_ok;
use crate::install::Platform;

pub fn backend() -> Result<Box<dyn Backend>, Failure> {
    #[cfg(target_os = "linux")]
    {
        super::systemd::Systemd::detect().map(|b| Box::new(b) as Box<dyn Backend>)
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(super::launchd::Launchd))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(super::windows::WindowsService))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Err(
            Failure::new("burrow service is not supported on this operating system")
                .hint("run burrow server or burrow client under your own service manager"),
        )
    }
}

#[cfg(unix)]
pub fn is_admin() -> bool {
    rustix::process::geteuid().is_root()
}

#[cfg(windows)]
pub fn is_admin() -> bool {
    run_ok("net", &["session"]) || run_ok("fltmc", &[])
}

pub fn require_admin() -> Result<(), Failure> {
    if is_admin() {
        return Ok(());
    }
    let hint = if cfg!(windows) {
        "run it from a terminal opened with \"Run as administrator\"".to_string()
    } else {
        let me = std::env::args().next().unwrap_or_else(|| "burrow".into());
        format!(
            "run: sudo {me} {}",
            std::env::args().skip(1).collect::<Vec<_>>().join(" ")
        )
    };
    Err(Failure::new("managing services needs administrator rights").hint(hint))
}

pub fn same_content(a: &Path, b: &Path) -> bool {
    let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
    len(a).is_some() && len(a) == len(b) && std::fs::read(a).ok() == std::fs::read(b).ok()
}

fn write_new_copy(from: &Path, tmp: &Path, executable: bool) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    let mode = if executable { 0o755 } else { 0o600 };
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = executable;
    let mut out = opts.open(tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        out.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    std::io::copy(&mut std::fs::File::open(from)?, &mut out)?;
    out.sync_all()
}

pub fn copy_file(from: &Path, to: &Path, executable: bool) -> Result<(), Failure> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))?;
    }
    let tmp = to.with_extension("burrow-new");
    let _ = std::fs::remove_file(&tmp);
    let written = write_new_copy(from, &tmp, executable);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(Failure::new(format!(
            "cannot copy {} to {}: {e}",
            from.display(),
            to.display()
        )));
    }
    replace(&tmp, to)
}

pub fn write_file(to: &Path, bytes: &[u8]) -> Result<(), Failure> {
    write_file_mode(to, bytes, 0o644)
}

pub fn write_file_mode(to: &Path, bytes: &[u8], mode: u32) -> Result<(), Failure> {
    use std::io::Write;
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))?;
    }
    let tmp = to.with_extension("burrow-new");
    let _ = std::fs::remove_file(&tmp);
    let written = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(Failure::new(format!("cannot write {}: {e}", to.display())));
    }
    replace(&tmp, to)
}

fn replace(tmp: &Path, to: &Path) -> Result<(), Failure> {
    let mut tries = 0;
    let mut moved_aside = None;
    loop {
        match std::fs::rename(tmp, to) {
            Ok(()) => return Ok(()),
            Err(e)
                if cfg!(windows)
                    && tries < 8
                    && e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(e)
                if cfg!(windows)
                    && tries == 8
                    && e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                tries += 1;
                let aside = to.with_extension("old");
                let _ = std::fs::remove_file(&aside);
                if std::fs::rename(to, &aside).is_ok() {
                    moved_aside = Some(aside);
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(tmp);
                if let Some(aside) = &moved_aside {
                    let _ = std::fs::rename(aside, to);
                }
                return Err(Failure::new(format!(
                    "cannot replace {}: {e}",
                    to.display()
                )));
            }
        }
    }
}

pub fn binary_version(path: &Path) -> String {
    Command::new(path)
        .arg("version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .trim_start_matches("burrow ")
                .to_string()
        })
        .unwrap_or_else(|| "unknown version".into())
}

pub fn run_as_service(name: &str) -> Result<u8, Failure> {
    #[cfg(windows)]
    {
        super::windows::dispatch(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Err(Failure::usage(
            "burrow service run is only used by the Windows service manager",
        ))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn private_file(path: &Path, owner: Option<&str>) -> Result<(), Failure> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let ids = owner.map(account_ids).transpose()?;
    let root_only_folder = path
        .parent()
        .and_then(|d| std::fs::symlink_metadata(d).ok())
        .is_some_and(|d| d.is_dir() && d.uid() == 0 && d.mode() & 0o022 == 0);
    if root_only_folder {
        if let (Some((uid, gid)), Some(account)) = (ids, owner) {
            std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(|e| {
                Failure::new(format!("cannot give {} to {account}: {e}", path.display()))
            })?;
        }
        return std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
            |e| Failure::new(format!("cannot set permissions on {}: {e}", path.display())),
        );
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| Failure::new(format!("cannot open {}: {e}", path.display())))?;
    let meta = file
        .metadata()
        .map_err(|e| Failure::new(format!("cannot read {}: {e}", path.display())))?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(Failure::new(format!(
            "{} is not a plain file with one link, so burrow will not secure it",
            path.display()
        ))
        .hint("remove it and run the command again"));
    }
    if let (Some((uid, gid)), Some(account)) = (ids, owner) {
        std::os::unix::fs::fchown(&file, Some(uid), Some(gid)).map_err(|e| {
            Failure::new(format!("cannot give {} to {account}: {e}", path.display()))
        })?;
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| Failure::new(format!("cannot set permissions on {}: {e}", path.display())))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn write_private(path: &Path, text: &str) -> Result<(), Failure> {
    write_file_mode(path, text.as_bytes(), 0o600)
}

pub struct Operation {
    _file: std::fs::File,
    path: PathBuf,
}

impl Operation {
    pub fn remove(self) {
        let Operation { _file, path } = self;
        drop(_file);
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(unix)]
fn names(path: &Path, file: &std::fs::File) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(path), file.metadata()) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn names(_path: &Path, _file: &std::fs::File) -> bool {
    true
}

pub const OPERATION_LOCK: &str = "operation.lock";

pub fn operation(out: &mut dyn std::io::Write) -> Result<Operation, Failure> {
    operation_in(
        &Platform::current().jobs_dir(),
        std::time::Duration::from_secs(300),
        out,
    )
}

fn operation_in(
    dir: &Path,
    patience: std::time::Duration,
    out: &mut dyn std::io::Write,
) -> Result<Operation, Failure> {
    let path = dir.join(OPERATION_LOCK);
    let deadline = std::time::Instant::now() + patience;
    let mut told = false;
    let mut replaced = 0;
    loop {
        crate::instance::private_dir(dir)
            .map_err(|e| Failure::new(format!("cannot prepare {}: {e}", dir.display())))?;
        if replaced > 20 {
            return Err(Failure::new(format!(
                "cannot keep a private lock file at {}",
                path.display()
            )));
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(0x1 | 0x2);
        }
        let file = opts
            .open(&path)
            .map_err(|e| Failure::new(format!("cannot open {}: {e}", path.display())))?;
        if !private_lock(&file) {
            let _ = std::fs::remove_file(&path);
            replaced += 1;
            continue;
        }
        match file.try_lock() {
            Ok(()) if names(&path, &file) => return Ok(Operation { _file: file, path }),
            Ok(()) => {
                replaced += 1;
                continue;
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Err(
                        Failure::new("another burrow service change is still running")
                            .hint("try again once it has finished"),
                    );
                }
                if !told {
                    let _ = writeln!(out, "waiting for another burrow service change to finish");
                    let _ = out.flush();
                    told = true;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(Failure::new(format!("cannot lock {}: {e}", path.display())));
            }
        }
    }
}

#[cfg(unix)]
fn private_lock(file: &std::fs::File) -> bool {
    use std::os::unix::fs::MetadataExt;
    file.metadata().is_ok_and(|m| {
        m.is_file()
            && m.nlink() == 1
            && m.uid() == rustix::process::geteuid().as_raw()
            && m.mode() & 0o077 == 0
    })
}

#[cfg(not(unix))]
fn private_lock(_file: &std::fs::File) -> bool {
    true
}

#[cfg(unix)]
pub fn read_plain_file(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.nlink() != 1 {
        return None;
    }
    let mut out = Vec::new();
    file.read_to_end(&mut out).ok()?;
    Some(out)
}

#[cfg(not(unix))]
pub fn read_plain_file(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return None;
    }
    std::fs::read(path).ok()
}

#[cfg(unix)]
pub fn give_plain_file(path: &Path, uid: u32, gid: u32) -> std::io::Result<bool> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Ok(false);
    }
    std::os::unix::fs::fchown(&file, Some(uid), Some(gid))?;
    Ok(true)
}

#[cfg(unix)]
pub fn give_back(path: &Path, from: u32, uid: u32, gid: u32) -> std::io::Result<bool> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.uid() != from {
        return Ok(false);
    }
    std::os::unix::fs::fchown(&file, Some(uid), Some(gid))?;
    Ok(true)
}

#[cfg(unix)]
pub fn make_dir(path: &Path, mode: u32, owner: Option<&str>) -> Result<(), Failure> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)
        .map_err(|e| Failure::new(format!("cannot create {}: {e}", path.display())))?;
    if let Some(account) = owner {
        let (uid, gid) = account_ids(account)?;
        std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(|e| {
            Failure::new(format!("cannot give {} to {account}: {e}", path.display()))
        })?;
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| Failure::new(format!("cannot set permissions on {}: {e}", path.display())))
}

#[cfg(target_os = "linux")]
pub fn all_paths_under(files: &[PathBuf], roots: &[&str]) -> bool {
    files.iter().any(|f| roots.iter().any(|r| f.starts_with(r)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn admin_means_the_effective_user_is_root() {
        let id = crate::base::process::run("id", &["-u"]).unwrap();
        assert_eq!(is_admin(), id.trim() == "0");
    }

    #[cfg(unix)]
    pub(crate) fn under_umask_077(test: &str, body: impl FnOnce()) {
        if std::env::var("BURROW_UMASK_TEST").as_deref() == Ok(test) {
            rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
            body();
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture", "--test-threads=1"])
            .env("BURROW_UMASK_TEST", test)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{test} under umask 077: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("1 passed"),
            "{test} did not run in the child"
        );
    }

    #[cfg(unix)]
    #[test]
    fn macos_service_folders_stay_reachable_under_a_strict_umask() {
        under_umask_077(
            "service::system::tests::macos_service_folders_stay_reachable_under_a_strict_umask",
            || {
                use std::os::unix::fs::PermissionsExt;
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path();
                let at = |p: &Path| root.join(p.strip_prefix("/").unwrap());
                let support = at(Path::new("/Library/Application Support"));
                std::fs::create_dir_all(&support).unwrap();
                for d in [root.join("Library"), support] {
                    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
                }
                let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
                let p = Platform::MacOs;
                assert!(p.launchd_folders().iter().any(|(d, m)| {
                    d == Path::new("/Library/Application Support/burrow/instances") && *m == 0o1777
                }));
                crate::instance::private_dir(&at(&p.jobs_dir())).unwrap();
                for (d, m) in p.launchd_folders() {
                    make_dir(&at(&d), m, None).unwrap();
                }
                let base = at(&p.state_dir()).parent().unwrap().to_path_buf();
                assert_eq!(mode(&base), 0o755, "the shared burrow folder");
                assert_eq!(mode(&at(&p.jobs_dir())), 0o700, "jobs stay private");
                assert_eq!(mode(&base.join("instances")), 0o1777);
            },
        );
    }

    #[test]
    fn service_changes_take_turns() {
        let dir = tempfile::tempdir().unwrap();
        let short = std::time::Duration::from_millis(300);
        let first = operation_in(dir.path(), short, &mut std::io::sink()).unwrap();
        assert!(operation_in(dir.path(), short, &mut std::io::sink()).is_err());
        drop(first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let lock = dir.path().join(OPERATION_LOCK);
            let mode = std::fs::metadata(&lock).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "only the owner may open the lock");
        }
        let second = operation_in(dir.path(), short, &mut std::io::sink()).unwrap();
        second.remove();
        assert!(!dir.path().join(OPERATION_LOCK).exists());
        let third = operation_in(dir.path(), short, &mut std::io::sink()).unwrap();
        assert!(operation_in(dir.path(), short, &mut std::io::sink()).is_err());
        drop(third);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let lock = dir.path().join(OPERATION_LOCK);
            std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644)).unwrap();
            let outsider = std::fs::File::open(&lock).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match outsider.try_lock() {
                    Ok(()) => break,
                    Err(std::fs::TryLockError::WouldBlock) => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "a child forked by another test still holds the lock"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(std::fs::TryLockError::Error(e)) => panic!("cannot lock: {e}"),
                }
            }
            let fresh = operation_in(dir.path(), short, &mut std::io::sink());
            assert!(fresh.is_ok(), "a lock others could open is replaced");
            drop(outsider);
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn securing_a_path_never_changes_what_a_link_there_points_to() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("com.example.daemon.plist");
        std::fs::write(&target, "x").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let link = dir.path().join("server.yaml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(private_file(&link, None).is_err());
        assert_eq!(mode(&target), 0o644);
        std::fs::remove_file(&link).unwrap();
        std::fs::hard_link(&target, &link).unwrap();
        assert!(private_file(&link, None).is_err());
        assert_eq!(mode(&target), 0o644);
        let plain = dir.path().join("plain.yaml");
        std::fs::write(&plain, "listen: 1\n").unwrap();
        private_file(&plain, None).unwrap();
        assert_eq!(mode(&plain), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn only_a_plain_file_with_one_link_is_read_as_root_would() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("server.yaml");
        std::fs::write(&plain, "listen: 1\n").unwrap();
        assert_eq!(read_plain_file(&plain).unwrap(), b"listen: 1\n");
        let victim = dir.path().join("shadow");
        std::fs::write(&victim, "x").unwrap();
        let link = dir.path().join("link.yaml");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        assert!(read_plain_file(&link).is_none());
        let hard = dir.path().join("hard.yaml");
        std::fs::hard_link(&victim, &hard).unwrap();
        assert!(read_plain_file(&hard).is_none());
        let fifo = dir.path().join("fifo.yaml");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(read_plain_file(&fifo).is_none());
        assert!(read_plain_file(&dir.path().join("gone")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn only_a_plain_file_with_one_link_changes_owner() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("a.log");
        std::fs::write(&plain, "x").unwrap();
        let (uid, gid) = {
            let m = std::fs::metadata(&plain).unwrap();
            (m.uid(), m.gid())
        };
        assert!(give_plain_file(&plain, uid, gid).unwrap());
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "x").unwrap();
        let linked = dir.path().join("b.log");
        std::fs::hard_link(&victim, &linked).unwrap();
        assert!(!give_plain_file(&linked, uid, gid).unwrap());
        let symlink = dir.path().join("c.log");
        std::os::unix::fs::symlink(&victim, &symlink).unwrap();
        assert!(give_plain_file(&symlink, uid, gid).is_err());
        let fifo = dir.path().join("d.log");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(!give_plain_file(&fifo, uid, gid).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn only_a_plain_file_the_account_owns_is_given_back() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("old.yaml");
        std::fs::write(&plain, "x").unwrap();
        let (uid, gid) = {
            let m = std::fs::metadata(&plain).unwrap();
            (m.uid(), m.gid())
        };
        assert!(!give_back(&plain, uid + 1, uid, gid).unwrap());
        assert!(give_back(&plain, uid, uid, gid).unwrap());
        let linked = dir.path().join("linked.yaml");
        std::fs::hard_link(&plain, &linked).unwrap();
        assert!(!give_back(&linked, uid, uid, gid).unwrap());
        let symlink = dir.path().join("link.yaml");
        std::os::unix::fs::symlink(&plain, &symlink).unwrap();
        assert!(give_back(&symlink, uid, uid, gid).is_err());
        assert!(give_back(&dir.path().join("missing"), uid, uid, gid).is_err());
    }

    #[test]
    fn a_rewritten_file_is_replaced_whole_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("burrow-x.json");
        write_file(&path, b"{\"old\": true}").unwrap();
        write_file(&path, b"{\"new\": true}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"new\": true}");
        assert!(!path.with_extension("burrow-new").exists());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn shared_folders_and_files_get_their_mode_whatever_the_umask() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("etc");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o750)).unwrap();
        make_dir(&shared, 0o755, None).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode(&shared), 0o755);
        let unit = shared.join("x.service");
        write_file(&unit, b"[Unit]").unwrap();
        assert_eq!(mode(&unit), 0o644);
        let config = shared.join("x.yaml");
        write_file_mode(&config, b"secret", 0o600).unwrap();
        assert_eq!(mode(&config), 0o600);
        let plist = shared.join("x.plist");
        std::fs::write(&plist, "a much longer previous job description").unwrap();
        write_private(&plist, "job").unwrap();
        assert_eq!(std::fs::read_to_string(&plist).unwrap(), "job");
        assert_eq!(mode(&plist), 0o600);
        assert!(!plist.with_extension("burrow-new").exists());
    }

    #[test]
    fn copies_are_private_and_leave_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.yaml");
        std::fs::write(&src, "listen: 7835\n").unwrap();
        let dst = dir.path().join("etc").join("server.yaml");
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(dst.with_extension("burrow-new"), "stale").unwrap();
        copy_file(&src, &dst, false).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "listen: 7835\n");
        assert!(!dst.with_extension("burrow-new").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o644)).unwrap();
            copy_file(&src, &dst, false).unwrap();
            assert_eq!(mode(&dst), 0o600);
            let bin = dir.path().join("burrow");
            copy_file(&src, &bin, true).unwrap();
            assert_eq!(mode(&bin), 0o755);
        }
        assert!(copy_file(&dir.path().join("missing"), &dst, false).is_err());
        assert!(!dst.with_extension("burrow-new").exists());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "listen: 7835\n");
    }
}
