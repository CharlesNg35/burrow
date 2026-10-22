use std::path::{Path, PathBuf};
use std::process::Command;

use super::Backend;
use crate::failure::Failure;

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

    pub fn launchd_folders(self) -> Vec<(PathBuf, u32)> {
        let state = self.state_dir();
        let base = state.parent().map(Path::to_path_buf).unwrap_or_default();
        vec![
            (self.config_dir(), 0o755),
            (base.clone(), 0o755),
            (state, 0o755),
            (base.join("instances"), 0o1777),
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

pub fn run(program: &str, args: &[&str]) -> Result<String, Failure> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Failure::new(format!("cannot run {program}: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.trim();
        Err(Failure::new(format!(
            "{program} {} failed: {}",
            args.join(" "),
            if msg.is_empty() { "no output" } else { msg }
        )))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub fn run_ok(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
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

#[cfg(unix)]
fn owned_by_root(p: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    #[cfg(test)]
    if tests::USER_WRITABLE.with(|w| w.borrow().iter().any(|d| d == p)) {
        return false;
    }
    std::fs::metadata(p).is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0)
}

#[cfg(unix)]
pub fn trusted_binary(path: &Path) -> bool {
    owned_by_root(path) && path.ancestors().skip(1).all(owned_by_root)
}

#[cfg(unix)]
pub fn trusted_folder(dir: &Path) -> bool {
    dir.ancestors().filter(|p| p.exists()).all(owned_by_root)
}

#[cfg(windows)]
pub fn trusted_folder(_dir: &Path) -> bool {
    true
}

#[cfg(windows)]
pub fn trusted_binary(path: &Path) -> bool {
    let named_burrow = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("burrow.exe"));
    let plain = path.components().all(|c| {
        !matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    });
    named_burrow
        && plain
        && std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .is_some_and(|root| path.starts_with(root))
}

pub fn canonical(p: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(p)
}

pub fn canonical_or_same(p: &Path) -> PathBuf {
    canonical(p).unwrap_or_else(|_| p.to_path_buf())
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

#[cfg(unix)]
pub fn account_ids(account: &str) -> Result<(u32, u32), Failure> {
    let read = |flag: &str| -> Result<u32, Failure> {
        run("id", &[flag, account])?
            .trim()
            .parse()
            .map_err(|_| Failure::new(format!("cannot read the ids of {account}")))
    };
    Ok((read("-u")?, read("-g")?))
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

    #[test]
    fn canonical_paths_are_plain_paths() {
        let dir = tempfile::tempdir().unwrap();
        let c = canonical(dir.path()).unwrap();
        assert!(c.is_absolute());
        assert!(!c.to_string_lossy().starts_with(r"\\?\"), "{c:?}");
        assert_eq!(c, canonical(&c).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn admin_means_the_effective_user_is_root() {
        let id = run("id", &["-u"]).unwrap();
        assert_eq!(is_admin(), id.trim() == "0");
    }

    #[cfg(unix)]
    thread_local! {
        pub(crate) static USER_WRITABLE: std::cell::RefCell<Vec<PathBuf>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    #[cfg(unix)]
    pub(crate) fn as_if_users_can_change(dir: &Path, body: impl FnOnce()) {
        USER_WRITABLE.with(|w| w.borrow_mut().push(dir.to_path_buf()));
        body();
        USER_WRITABLE.with(|w| w.borrow_mut().clear());
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

    #[cfg(unix)]
    #[test]
    fn a_folder_users_can_change_is_not_trusted_for_the_service_binary() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!trusted_folder(dir.path()));
        assert!(!trusted_folder(&dir.path().join("bin")));
        assert!(trusted_folder(Path::new("/usr")));
        assert!(trusted_folder(Path::new("/usr/no-such-folder/bin")));
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
