use std::path::{Path, PathBuf};

use super::save;

#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    Synced(PathBuf),
    Differs(PathBuf),
    Alone,
}

fn canonical(path: &Path) -> PathBuf {
    crate::base::fs::canonical_or_same(path)
}

pub fn follow(source: &Path, installed: &Path, before: &str, text: &str) -> Source {
    let source = canonical(source);
    if source == canonical(installed) || !source.is_file() {
        return Source::Alone;
    }
    match std::fs::read_to_string(&source) {
        Ok(current) if current == before => match save::write_back(&source, before, text) {
            Ok(()) => Source::Synced(source),
            Err(_) => Source::Differs(source),
        },
        Ok(_) => Source::Differs(source),
        Err(_) => Source::Alone,
    }
}

pub fn of_service(service: &str, installed: &Path, before: &str, text: &str) -> Source {
    match crate::service::resolve(Some(service)) {
        Ok(m) => follow(&m.source_config, installed, before, text),
        Err(_) => Source::Alone,
    }
}

pub fn installed_from(path: &Path) -> Vec<String> {
    let real = canonical(path);
    crate::service::installed()
        .into_iter()
        .filter(|i| canonical(&i.manifest.source_config) == real)
        .map(|i| i.manifest.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_source_follows_only_when_it_matched_the_installed_copy() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("installed.yaml");
        let source = dir.path().join("client.yaml");
        std::fs::write(&installed, "name: office\n").unwrap();
        std::fs::write(&source, "name: home\n").unwrap();
        assert_eq!(
            follow(&source, &installed, "name: home\n", "name: office\n"),
            Source::Synced(canonical(&source))
        );
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "name: office\n");
        assert_eq!(
            follow(&source, &installed, "name: home\n", "name: lab\n"),
            Source::Differs(canonical(&source))
        );
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "name: office\n");
        assert_eq!(follow(&installed, &installed, "a", "b"), Source::Alone);
        assert_eq!(
            follow(&dir.path().join("gone.yaml"), &installed, "a", "b"),
            Source::Alone
        );
    }
}
