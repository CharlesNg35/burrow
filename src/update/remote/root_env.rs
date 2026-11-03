use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use crate::install::Manifest;

pub const TRUST_ENV: &str = "BURROW_ADMIN_TEST_TRUST_ENV";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owned {
    pub uid: u32,
    pub mode: u32,
}

pub fn judge(file: Owned, folder: Owned) -> Result<(), String> {
    if file.uid != 0 {
        return Err("it does not belong to root".into());
    }
    if file.mode & 0o022 != 0 {
        return Err("accounts other than root may change it".into());
    }
    if folder.uid != 0 || folder.mode & 0o022 != 0 {
        return Err("its folder is not root's alone".into());
    }
    Ok(())
}

#[cfg(unix)]
fn check(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let file = crate::base::fs::open_plain(path)
        .map_err(|e| e.to_string())?
        .ok_or("it is not a plain file with one link")?;
    let f = file.metadata().map_err(|e| e.to_string())?;
    let dir = path.parent().ok_or("it has no folder")?;
    let d = std::fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
    if !d.is_dir() {
        return Err("its folder is a link".into());
    }
    judge(
        Owned {
            uid: f.uid(),
            mode: f.mode(),
        },
        Owned {
            uid: d.uid(),
            mode: d.mode(),
        },
    )
}

#[cfg(not(unix))]
fn check(_path: &Path) -> Result<(), String> {
    Err("burrow does not check who may change it on this system".into())
}

pub fn trusted(env: &BTreeMap<String, String>, checked: Result<(), String>) -> Result<(), String> {
    match checked {
        Err(_)
            if cfg!(feature = "admin-test-hooks")
                && env.get(TRUST_ENV).is_some_and(|v| v == "1") =>
        {
            Ok(())
        }
        other => other,
    }
}

pub fn for_root(m: &Manifest, out: &mut dyn Write) -> BTreeMap<String, String> {
    let Some(path) = &m.env_file else {
        return BTreeMap::new();
    };
    let env = crate::service::env_of(m);
    match trusted(&env, check(path)) {
        Ok(()) => env,
        Err(why) => {
            let _ = writeln!(
                out,
                "not reading {} for the update: {why}; the official releases are used",
                path.display()
            );
            BTreeMap::new()
        }
    }
}
