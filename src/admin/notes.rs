use std::io::Read;
use std::path::Path;

use burrow_admin_proto::msg::AuditEntry;

const PREFIX: &str = "audit-";
const SUFFIX: &str = ".json";

pub fn name_for(version: &str) -> Option<String> {
    let stem = version.split('.').next().filter(|s| !s.is_empty())?;
    Some(format!("{PREFIX}{stem}{SUFFIX}"))
}

pub fn stale(names: &[String], is_version: impl Fn(&str) -> bool) -> Vec<String> {
    let kept: Vec<String> = names
        .iter()
        .filter(|n| is_version(n))
        .filter_map(|n| name_for(n))
        .collect();
    names
        .iter()
        .filter(|n| n.starts_with(PREFIX) && n.ends_with(SUFFIX) && !kept.contains(n))
        .cloned()
        .collect()
}

pub fn body(entry: &AuditEntry) -> Option<String> {
    serde_json::to_string_pretty(&AuditEntry {
        id: 0,
        ..entry.clone()
    })
    .ok()
}

pub fn read(path: &Path, rev: &str) -> Option<AuditEntry> {
    let mut file = crate::base::fs::open_plain(path).ok().flatten()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    serde_json::from_str::<AuditEntry>(&text)
        .ok()
        .filter(|e| e.rev == rev)
}

#[cfg(test)]
mod tests {
    use burrow_admin_proto::msg::AuditEntry;

    use crate::admin::adapters::FileStore;
    use crate::admin::core::ports::Store;
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
        assert!(store.noted(&revs[4]).is_none());
        let noted = store.noted(&revs[24]).unwrap();
        assert_eq!((noted.visitor.as_str(), noted.id), ("ops", 0));
        store.keep(&revs[10], &Docs::one("server.yaml", "listen: 10\n"));
        assert!(
            store.noted(&revs[10]).is_none(),
            "a version kept again is not the change the note describes"
        );
        assert_eq!(names(&path, "audit-").len(), 19);
        store.note(&entry(&text::rev("listen: 99\n")));
        assert_eq!(names(&path, "audit-").len(), 19);
        #[cfg(unix)]
        for name in names(&path, "audit-") {
            use std::os::unix::fs::PermissionsExt;
            let file = crate::config::save::history_dir_of(&path).join(name);
            let mode = std::fs::metadata(file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
