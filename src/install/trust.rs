use std::path::Path;

use super::{Manifest, Platform};

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
            .map(std::path::PathBuf::from)
            .is_some_and(|root| path.starts_with(root))
}

pub(crate) fn binary_trusted(m: &Manifest, p: Platform) -> bool {
    m.binary == p.service_binary()
        || ((cfg!(not(windows)) || !m.copied_binary) && trusted_binary(&m.binary))
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;

    thread_local! {
        pub(crate) static USER_WRITABLE: std::cell::RefCell<Vec<std::path::PathBuf>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    pub(crate) fn as_if_users_can_change(dir: &Path, body: impl FnOnce()) {
        USER_WRITABLE.with(|w| w.borrow_mut().push(dir.to_path_buf()));
        body();
        USER_WRITABLE.with(|w| w.borrow_mut().clear());
    }

    #[test]
    fn a_folder_users_can_change_is_not_trusted_for_the_service_binary() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!trusted_folder(dir.path()));
        assert!(!trusted_folder(&dir.path().join("bin")));
        assert!(trusted_folder(Path::new("/usr")));
        assert!(trusted_folder(Path::new("/usr/no-such-folder/bin")));
    }
}
