use std::path::{Path, PathBuf};

use crate::admin_link::store;
use crate::admin_link::token::Token;
use crate::base::failure::Failure;

pub const FILE_PREFIX: &str = "mcp-";
pub const FILE_EXTENSION: &str = ".token";

pub fn default_path(config: &Path) -> Result<PathBuf, Failure> {
    let dir = store::dir().ok_or_else(|| {
        Failure::new("cannot find a private folder for the burrow mcp token")
            .hint("pass --token-file FILE")
    })?;
    store::make_private_dir(&dir)
        .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))?;
    Ok(store::file_for(&dir, config, FILE_PREFIX, FILE_EXTENSION))
}

#[cfg(unix)]
fn private(path: &Path) -> Result<(), Failure> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|e| Failure::new(format!("cannot read {}: {e}", path.display())))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(Failure::new(format!(
            "{} can be read by other users, so its token is not used",
            path.display()
        ))
        .hint(format!(
            "run: chmod 600 {}, or delete it to make a new token",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn private(_: &Path) -> Result<(), Failure> {
    Ok(())
}

fn make(path: &Path) -> Result<Token, Failure> {
    let t = Token::generate();
    crate::base::fs::write_private(path, &format!("{}\n", t.as_str()))
        .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))?;
    Ok(t)
}

pub fn replace(path: &Path) -> Result<Token, Failure> {
    load_or_create(path)?;
    make(path)
}

pub fn matches(path: &Path, given: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| {
        let t = t.trim();
        Token::parse(t).is_some_and(|t| private(path).is_ok() && t.matches(given))
    })
}

pub fn load_or_create(path: &Path) -> Result<Token, Failure> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let Some(t) = Token::parse(text.trim()) else {
                return Err(Failure::new(format!(
                    "{} is not a burrow mcp token file",
                    path.display()
                ))
                .hint("pass another --token-file, or delete the file to make a new token"));
            };
            private(path)?;
            Ok(t)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => make(path),
        Err(e) => Err(Failure::new(format!(
            "cannot read {}: {}",
            path.display(),
            crate::base::failure::file_reason(&e)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_made_once_kept_private_and_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.token");
        let first = load_or_create(&path).unwrap();
        let again = load_or_create(&path).unwrap();
        assert_eq!(first.as_str(), again.as_str());
        assert_eq!(first.as_str().len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_new_token_replaces_the_old_one_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.token");
        let old = load_or_create(&path).unwrap();
        assert!(matches(&path, old.as_str()));
        let new = replace(&path).unwrap();
        assert_ne!(old.as_str(), new.as_str());
        assert!(!matches(&path, old.as_str()));
        assert!(matches(&path, new.as_str()));
        assert!(!matches(&path, ""));
        std::fs::remove_file(&path).unwrap();
        assert!(!matches(&path, new.as_str()), "no file, no access");
    }

    #[test]
    fn a_file_that_is_not_a_token_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        crate::base::fs::write_private(&path, "my notes\n").unwrap();
        let e = load_or_create(&path).unwrap_err();
        assert!(
            e.message.contains("is not a burrow mcp token file"),
            "{}",
            e.message
        );
        assert!(replace(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "my notes\n");
    }

    #[test]
    fn burrow_admin_never_sweeps_the_token_away() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("ops.yaml");
        std::fs::write(&config, "server: x\n").unwrap();
        let path = store::file_for(dir.path(), &config, FILE_PREFIX, FILE_EXTENSION);
        let t = load_or_create(&path).unwrap();
        crate::admin_local::session::sweep(dir.path(), u64::MAX / 2);
        assert_eq!(load_or_create(&path).unwrap().as_str(), t.as_str());
    }

    #[cfg(unix)]
    #[test]
    fn a_token_others_can_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.token");
        load_or_create(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let e = load_or_create(&path).unwrap_err();
        assert!(e.hint.unwrap().contains("chmod 600"));
    }
}
