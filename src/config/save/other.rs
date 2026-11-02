use std::io::Write;
use std::path::{Path, PathBuf};

use super::{Refused, spare_prefix};

pub struct Pending {
    real: PathBuf,
    spare: PathBuf,
    text: Option<String>,
}

impl Pending {
    pub fn commit(&self) -> std::io::Result<()> {
        match &self.text {
            Some(text) => {
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&self.real)?;
                f.write_all(text.as_bytes())?;
                f.sync_all()?;
                drop(f);
                std::fs::remove_file(&self.spare)
            }
            None => {
                if self.real.exists() {
                    return Err(std::io::ErrorKind::AlreadyExists.into());
                }
                std::fs::rename(&self.spare, &self.real)
            }
        }
    }

    pub fn discard(&self) {
        let _ = std::fs::remove_file(&self.spare);
    }
}

fn spare_for(real: &Path) -> Result<PathBuf, Refused> {
    let (Some(folder), Some(name)) = (real.parent(), real.file_name()) else {
        return Err(Refused::Io(std::io::ErrorKind::InvalidInput.into()));
    };
    Ok(folder.join(format!(
        "{}{}",
        spare_prefix(&name.to_string_lossy()),
        std::process::id()
    )))
}

fn write_new(spare: &Path, text: &str) -> std::io::Result<()> {
    let _ = std::fs::remove_file(spare);
    let mut copy = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(spare)?;
    copy.write_all(text.as_bytes())?;
    copy.sync_all()
}

pub fn stage_replace(path: &Path, before: &str, text: &str) -> Result<Pending, Refused> {
    if std::fs::read_to_string(path)? != before {
        return Err(Refused::Changed);
    }
    let real = std::fs::canonicalize(path)?;
    let spare = spare_for(&real)?;
    write_new(&spare, text)?;
    Ok(Pending {
        real,
        spare,
        text: Some(text.to_string()),
    })
}

pub fn stage_create(path: &Path, text: &str, _like: &Path) -> Result<Pending, Refused> {
    let parent = path
        .parent()
        .ok_or(Refused::Io(std::io::ErrorKind::InvalidInput.into()))?;
    if let Err(e) = std::fs::create_dir(parent)
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(Refused::Io(e));
    }
    let name = path
        .file_name()
        .ok_or(Refused::Io(std::io::ErrorKind::InvalidInput.into()))?;
    let real = std::fs::canonicalize(parent)?.join(name);
    if real.exists() {
        return Err(Refused::Changed);
    }
    let spare = spare_for(&real)?;
    write_new(&spare, text)?;
    Ok(Pending {
        real,
        spare,
        text: None,
    })
}
