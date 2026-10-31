use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::storage::{account_dir, name_set, write_private};

pub const LIMIT: usize = 4;
pub const WINDOW_SECS: u64 = 7 * burrow_syntax::duration::DAY_SECS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issued {
    pub names: Vec<String>,
    pub at: u64,
    pub ari: bool,
}

pub fn path(state: &Path, directory: &str) -> PathBuf {
    account_dir(state, directory).join("issued.json")
}

pub fn load(path: &Path) -> Vec<Issued> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn next_allowed(list: &[Issued], names: &[String], now: u64) -> Option<u64> {
    let set = name_set(names);
    let mut recent: Vec<u64> = list
        .iter()
        .filter(|i| !i.ari && name_set(&i.names) == set && now < i.at + WINDOW_SECS)
        .map(|i| i.at)
        .collect();
    if recent.len() < LIMIT {
        return None;
    }
    recent.sort_unstable();
    Some(recent[recent.len() - LIMIT] + WINDOW_SECS)
}

pub fn record(path: &Path, names: &[String], ari: bool, now: u64) -> std::io::Result<()> {
    let mut list = load(path);
    list.retain(|i| now < i.at + WINDOW_SECS);
    list.push(Issued {
        names: name_set(names),
        at: now,
        ari,
    });
    let bytes = serde_json::to_vec_pretty(&list).map_err(std::io::Error::other)?;
    write_private(path, &bytes)
}

pub fn refusal(names: &[String], next: u64) -> String {
    let when = httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(next));
    format!(
        "{LIMIT} new certificates for {} were issued in the last 7 days; to stay under Let's Encrypt's limit of 5 a week for the same names, burrow orders the next one after {when}",
        name_set(names).join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn four_new_certificates_for_the_same_names_in_a_week_stop_the_fifth_and_ari_is_exempt() {
        let state = tempfile::tempdir().unwrap();
        let p = path(state.path(), "https://acme.test/dir");
        let day = burrow_syntax::duration::DAY_SECS;
        let ab = names(&["b.example.com", "a.example.com"]);
        let now = 1_800_000_000;
        for i in 0..3 {
            record(&p, &ab, false, now + i * day).unwrap();
        }
        record(&p, &ab, true, now + 3 * day).unwrap();
        record(&p, &names(&["a.example.com"]), false, now + 3 * day).unwrap();
        let later = now + 4 * day;
        assert_eq!(
            next_allowed(&load(&p), &ab, later),
            None,
            "an ARI renewal and another set do not count"
        );
        record(&p, &ab, false, later).unwrap();
        let reversed = names(&["a.example.com", "b.example.com"]);
        assert_eq!(
            next_allowed(&load(&p), &reversed, later),
            Some(now + 7 * day)
        );
        assert_eq!(
            next_allowed(&load(&p), &ab, now + 7 * day),
            None,
            "the oldest left the window"
        );
        let text = refusal(&ab, now + 7 * day);
        assert!(
            text.contains("a.example.com, b.example.com") && text.ends_with("GMT"),
            "{text}"
        );
    }

    #[test]
    fn the_record_survives_a_restart_and_forgets_what_left_the_window() {
        let state = tempfile::tempdir().unwrap();
        let p = path(state.path(), "https://acme.test/dir");
        let a = names(&["a.example.com"]);
        record(&p, &a, false, 100).unwrap();
        record(&p, &a, false, 100 + WINDOW_SECS + 1).unwrap();
        let list = load(&p);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].at, 100 + WINDOW_SECS + 1);
        std::fs::write(&p, "not json").unwrap();
        assert!(load(&p).is_empty());
    }
}
