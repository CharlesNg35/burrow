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

pub fn run_ok(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(unix)]
pub fn is_admin() -> bool {
    run("id", &["-u"]).is_ok_and(|s| s.trim() == "0")
}

#[cfg(windows)]
pub fn is_admin() -> bool {
    run_ok("net", &["session"])
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
pub fn trusted_binary(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let owned_by_root =
        |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0);
    owned_by_root(path) && path.ancestors().skip(1).all(owned_by_root)
}

#[cfg(windows)]
pub fn trusted_binary(path: &Path) -> bool {
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let windir = std::env::var_os("SystemRoot").map(PathBuf::from);
    [program_files, windir]
        .into_iter()
        .flatten()
        .any(|root| path.starts_with(root))
}

pub fn canonical(p: &Path) -> std::io::Result<PathBuf> {
    let c = std::fs::canonicalize(p)?;
    #[cfg(windows)]
    if let Some(rest) = c.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
        && !rest.starts_with("UNC")
    {
        return Ok(PathBuf::from(rest));
    }
    Ok(c)
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
    let mut tries = 0;
    loop {
        match std::fs::rename(&tmp, to) {
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
                let _ = std::fs::rename(to, &aside);
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
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

#[cfg(unix)]
pub fn private_file(path: &Path, owner: Option<&str>) -> Result<(), Failure> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(account) = owner {
        let (uid, gid) = account_ids(account)?;
        std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(|e| {
            Failure::new(format!("cannot give {} to {account}: {e}", path.display()))
        })?;
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| Failure::new(format!("cannot set permissions on {}: {e}", path.display())))
}

#[cfg(unix)]
pub fn write_private(path: &Path, text: &str) -> Result<(), Failure> {
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))?;
    std::io::Write::write_all(&mut f, text.as_bytes())
        .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))
}

#[cfg(target_os = "macos")]
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
mod tests {
    use super::*;

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
