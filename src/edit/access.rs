use std::path::Path;

#[cfg(unix)]
pub fn other_owner(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata(path).ok()?.uid();
    let me = std::env::var("SUDO_UID")
        .ok()
        .filter(|_| rustix::process::geteuid().is_root())
        .and_then(|u| u.parse().ok())
        .unwrap_or_else(|| rustix::process::geteuid().as_raw());
    (uid != me).then(|| name_of(uid).unwrap_or_else(|| uid.to_string()))
}

#[cfg(not(unix))]
pub fn other_owner(_path: &Path) -> Option<String> {
    None
}

#[cfg(unix)]
fn name_of(uid: u32) -> Option<String> {
    let out = std::process::Command::new("id")
        .args(["-nu", &uid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

#[cfg(unix)]
pub fn can_write(path: &Path) -> bool {
    use rustix::fs::{Access, access};
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    access(path, Access::WRITE_OK).is_ok() && access(dir, Access::WRITE_OK).is_ok()
}

#[cfg(not(unix))]
pub fn can_write(path: &Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn my_own_files_are_writable_and_need_no_note() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.yaml");
        std::fs::write(&path, "x").unwrap();
        assert!(can_write(&path));
        assert!(other_owner(&path).is_none() || std::env::var("SUDO_UID").is_ok());
        if !rustix::process::geteuid().is_root() {
            assert!(!can_write(Path::new("/etc/passwd")));
            assert_eq!(
                other_owner(Path::new("/etc/passwd")).as_deref(),
                Some("root")
            );
        }
    }
}
