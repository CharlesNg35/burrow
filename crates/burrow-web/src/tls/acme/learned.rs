use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::storage::{account_dir, write_private};
use crate::tls::CertCheck;

pub fn path(state: &Path, directory: &str) -> PathBuf {
    account_dir(state, directory).join("checks.json")
}

fn word(c: CertCheck) -> &'static str {
    match c {
        CertCheck::Tls => "tls",
        CertCheck::Http => "http",
    }
}

pub fn load(path: &Path) -> BTreeMap<String, CertCheck> {
    let raw: BTreeMap<String, String> = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    raw.into_iter()
        .filter_map(|(n, c)| match c.as_str() {
            "tls" => Some((n, CertCheck::Tls)),
            "http" => Some((n, CertCheck::Http)),
            _ => None,
        })
        .collect()
}

pub fn remember(path: &Path, names: &[String], check: CertCheck) -> std::io::Result<()> {
    let mut all: BTreeMap<String, String> = load(path)
        .into_iter()
        .map(|(n, c)| (n, word(c).to_string()))
        .collect();
    for n in names {
        all.insert(n.to_ascii_lowercase(), word(check).to_string());
    }
    let bytes = serde_json::to_vec_pretty(&all).map_err(std::io::Error::other)?;
    write_private(path, &bytes)
}

pub fn first(
    learned: &BTreeMap<String, CertCheck>,
    names: &[String],
    allowed: &[CertCheck],
) -> Option<CertCheck> {
    let mut seen = names
        .iter()
        .filter_map(|n| learned.get(&n.to_ascii_lowercase()).copied());
    let one = seen.next()?;
    (seen.all(|c| c == one) && allowed.contains(&one)).then_some(one)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_that_worked_is_remembered_per_name_across_restarts() {
        let state = tempfile::tempdir().unwrap();
        let p = path(state.path(), "https://acme.test/dir");
        let names = |v: &[&str]| v.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let both = [CertCheck::Tls, CertCheck::Http];
        assert_eq!(first(&load(&p), &names(&["a.example.com"]), &both), None);
        remember(
            &p,
            &names(&["A.example.com", "b.example.com"]),
            CertCheck::Http,
        )
        .unwrap();
        let learned = load(&p);
        assert_eq!(
            first(&learned, &names(&["a.example.com", "b.example.com"]), &both),
            Some(CertCheck::Http)
        );
        assert_eq!(
            first(&learned, &names(&["a.example.com"]), &[CertCheck::Tls]),
            None,
            "only within the allowed checks"
        );
        remember(&p, &names(&["b.example.com"]), CertCheck::Tls).unwrap();
        assert_eq!(
            first(
                &load(&p),
                &names(&["a.example.com", "b.example.com"]),
                &both
            ),
            None,
            "names disagree"
        );
        std::fs::write(&p, "garbage").unwrap();
        assert!(load(&p).is_empty());
    }
}
