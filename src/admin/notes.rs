use std::io::Read;
use std::path::Path;

use burrow_admin_proto::msg::AuditEntry;
use serde::{Deserialize, Serialize};

use super::core::ports::Noted;
use super::core::text;

const PREFIX: &str = "audit-";
const SUFFIX: &str = ".json";

#[derive(Serialize, Deserialize)]
struct Line {
    kept_as: String,
    #[serde(flatten)]
    entry: AuditEntry,
}

pub fn name_for(rev: &str) -> Option<String> {
    Some(format!("{PREFIX}{}{SUFFIX}", text::rev_hex(rev)?))
}

pub fn stem(version: &str) -> Option<&str> {
    version.split('.').next().filter(|s| !s.is_empty())
}

pub fn stale(names: &[String], rev_of: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let kept: Vec<String> = names
        .iter()
        .filter_map(|n| rev_of(n))
        .filter_map(|r| name_for(&r))
        .collect();
    names
        .iter()
        .filter(|n| n.starts_with(PREFIX) && n.ends_with(SUFFIX) && !kept.contains(n))
        .cloned()
        .collect()
}

fn lines(path: &Path, rev: &str) -> Vec<Line> {
    let Some(mut file) = crate::base::fs::open_plain(path).ok().flatten() else {
        return Vec::new();
    };
    let mut text = String::new();
    if file.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    let mut lines: Vec<Line> = serde_json::from_str(&text).unwrap_or_default();
    lines.retain(|l| l.entry.rev == rev);
    lines
}

pub fn added(path: &Path, kept_as: &str, entry: &AuditEntry, cap: usize) -> Option<String> {
    let mut all = lines(path, &entry.rev);
    all.push(Line {
        kept_as: kept_as.to_string(),
        entry: entry.clone(),
    });
    let extra = all.len().saturating_sub(cap);
    all.drain(..extra);
    serde_json::to_string_pretty(&all).ok()
}

fn placed_at(stem: &str) -> u128 {
    stem.split_once('-')
        .and_then(|(nanos, _)| nanos.parse().ok())
        .unwrap_or(0)
}

pub fn read(path: &Path, kept_as: &str, rev: &str) -> Noted {
    let all = lines(path, rev);
    Noted {
        kept: (all.last().map(|l| l.kept_as.as_str()) != Some(kept_as)).then(|| placed_at(kept_as)),
        entries: all
            .into_iter()
            .map(|l| (placed_at(&l.kept_as), l.entry))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use burrow_admin_proto::msg::AuditEntry;

    use crate::admin::adapters::FileStore;
    use crate::admin::core::ports::{Store, Version};
    use crate::admin::core::text;
    use crate::config::docs::Docs;

    fn entry(rev: &str) -> AuditEntry {
        AuditEntry {
            id: 7,
            at: 1,
            visitor: "ops".into(),
            from: "203.0.113.7".into(),
            base: String::new(),
            rev: rev.into(),
            changes: Vec::new(),
            result: "applied".into(),
            revertible: true,
            details_kept: true,
            reverts: None,
        }
    }

    fn version(rev: &str) -> Version {
        Version {
            rev: rev.into(),
            at: 0,
        }
    }

    fn names(store_path: &std::path::Path, prefix: &str) -> Vec<String> {
        std::fs::read_dir(crate::config::save::history_dir_of(store_path))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(prefix))
            .collect()
    }

    #[test]
    fn notes_go_with_their_versions_and_never_count_as_versions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.yaml");
        std::fs::write(&path, "listen: 1\n").unwrap();
        let store = FileStore::new(&path);
        let mut revs = Vec::new();
        for i in 0..25 {
            let body = format!("listen: {i}\n");
            let rev = text::rev(&body);
            store.keep(&rev, &Docs::one("server.yaml", body));
            store.note(&entry(&rev));
            revs.push(rev);
        }
        assert_eq!(store.versions().len(), super::super::adapters::KEEP);
        assert_eq!(names(&path, "audit-").len(), 20);
        assert_eq!(names(&path, ".").len(), 0, "no drafts are left behind");
        assert!(store.noted(&version(&revs[4])).entries.is_empty());
        store.note(&entry(&revs[24]));
        let noted = store.noted(&version(&revs[24]));
        assert_eq!(noted.kept, None);
        assert_eq!(
            noted.entries.len(),
            2,
            "every change that made it, oldest first"
        );
        assert_eq!(
            (noted.entries[0].1.visitor.as_str(), noted.entries[0].1.id),
            ("ops", 7)
        );
        store.keep(&revs[10], &Docs::one("server.yaml", "listen: 10\n"));
        let again = store.noted(&version(&revs[10]));
        assert!(
            again.kept.is_some() && again.entries.len() == 1,
            "a version kept again by hand keeps its changes and reads as kept since"
        );
        assert_eq!(names(&path, "audit-").len(), 20);
        store.note(&entry(&text::rev("listen: 99\n")));
        assert_eq!(names(&path, "audit-").len(), 20);
        #[cfg(unix)]
        for name in names(&path, "audit-") {
            use std::os::unix::fs::PermissionsExt;
            let file = crate::config::save::history_dir_of(&path).join(name);
            let mode = std::fs::metadata(file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_record_keeps_the_newest_changes_up_to_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit-ab.json");
        let rev = text::rev("listen: 1\n");
        for at in 0..3 {
            let body = super::added(&path, "1-ab", &AuditEntry { at, ..entry(&rev) }, 2).unwrap();
            std::fs::write(&path, body).unwrap();
        }
        let noted = super::read(&path, "1-ab", &rev);
        let ats: Vec<(u128, u64)> = noted.entries.iter().map(|(o, e)| (*o, e.at)).collect();
        assert_eq!((ats, noted.kept), (vec![(1, 1), (1, 2)], None));
        assert_eq!(super::read(&path, "2-ab", &rev).kept, Some(2));
        assert!(super::read(&path, "1-ab", "b2s:other").entries.is_empty());
    }
}
