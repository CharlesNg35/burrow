use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub struct Draft {
    dir: PathBuf,
    file: PathBuf,
    keep: bool,
}

impl Draft {
    pub fn new(name: &OsStr, text: &str) -> std::io::Result<Draft> {
        Draft::new_in(&std::env::temp_dir(), name, text)
    }

    pub fn new_in(parent: &Path, name: &OsStr, text: &str) -> std::io::Result<Draft> {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).map_err(std::io::Error::other)?;
        let tag = hex::encode(random);
        let dir = parent.join(format!("burrow-edit-{tag}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir(&dir)?;
        let draft = Draft {
            file: dir.join(name),
            dir,
            keep: false,
        };
        crate::base::fs::write_private(&draft.file, text)?;
        Ok(draft)
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    pub fn read(&self) -> std::io::Result<String> {
        std::fs::read_to_string(&self.file)
    }

    pub fn keep(mut self) -> PathBuf {
        self.keep = true;
        self.file.clone()
    }
}

impl Drop for Draft {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.file);
            let _ = std::fs::remove_dir(&self.dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_is_private_and_gone_unless_kept() {
        let parent = tempfile::tempdir().unwrap();
        let draft =
            Draft::new_in(parent.path(), OsStr::new("client.yaml"), "name: home\n").unwrap();
        assert_eq!(draft.read().unwrap(), "name: home\n");
        assert_eq!(draft.path().file_name().unwrap(), "client.yaml");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = draft.path().parent().unwrap();
            assert_eq!(
                std::fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            let file = std::fs::metadata(draft.path()).unwrap();
            assert_eq!(file.permissions().mode() & 0o777, 0o600);
        }
        let dir = draft.path().parent().unwrap().to_path_buf();
        drop(draft);
        assert!(!dir.exists());
        let kept = Draft::new_in(parent.path(), OsStr::new("x.yaml"), "a")
            .unwrap()
            .keep();
        assert!(kept.exists());
    }
}
