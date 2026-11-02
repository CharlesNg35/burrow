use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::wire::protocol::{LastUpdate, Outcome};

const MAX_RECORD: u64 = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Request {
    pub id: u32,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Record {
    pub id: u32,
    pub from: String,
    pub to: String,
    pub outcome: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub config: PathBuf,
    #[serde(default)]
    pub starts: u32,
}

impl Record {
    pub fn new(id: u32, from: &str, to: &str, config: &Path) -> Record {
        Record {
            id,
            from: from.into(),
            to: to.into(),
            outcome: Outcome::Trying.wire().into(),
            detail: String::new(),
            config: config.to_path_buf(),
            starts: 0,
        }
    }

    pub fn outcome(&self) -> Option<Outcome> {
        Outcome::parse(&self.outcome)
    }

    pub fn ended(mut self, outcome: Outcome, detail: impl Into<String>) -> Record {
        self.outcome = outcome.wire().into();
        self.detail = detail.into();
        self
    }

    pub fn last(&self) -> Option<LastUpdate> {
        Some(LastUpdate {
            id: self.id,
            outcome: self.outcome()?,
            from: self.from.clone(),
            to: self.to.clone(),
            detail: self.detail.clone(),
        })
    }
}

pub fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    file.take(MAX_RECORD).read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write<T: Serialize>(path: &Path, value: &T, mode: u32) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_vec_pretty(value).expect("records serialize");
    crate::base::fs::write_atomic(path, &text, mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_reads_back_and_reports_its_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.json");
        let r = Record::new(
            3,
            "0.2.1",
            "0.2.2",
            Path::new("/etc/burrow/nas/client.yaml"),
        );
        write(&path, &r, 0o600).unwrap();
        let back: Record = read(&path).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.last().unwrap().outcome, Outcome::Trying);
        let done = back.ended(Outcome::RolledBack, "it did not connect");
        assert_eq!(done.last().unwrap().detail, "it did not connect");
        std::fs::write(&path, "x".repeat(20_000)).unwrap();
        assert_eq!(read::<Record>(&path), None);
    }
}
