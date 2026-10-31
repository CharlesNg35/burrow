use burrow_admin_proto::features::HISTORY_IDS;
use burrow_admin_proto::msg::{AuditEntry, HistoryRequest, Welcome};

pub fn has_ids(welcome: Option<&Welcome>) -> bool {
    welcome.is_some_and(|w| w.has(HISTORY_IDS))
}

pub const OLDER_REPEAT: &str = "This server is older and keeps the changed lines once per version; the newest entry with this version shows them.";

fn local_id(rev: &str, at: u64, nth: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in rev.bytes().chain(at.to_le_bytes()).chain(nth.to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h.max(1)
}

pub fn with_ids(mut entries: Vec<AuditEntry>) -> Vec<AuditEntry> {
    let mut seen: Vec<(String, u64)> = Vec::new();
    for e in entries.iter_mut().rev() {
        let key = (e.rev.clone(), e.at);
        let nth = seen.iter().filter(|k| **k == key).count() as u64;
        seen.push(key);
        if e.id == 0 {
            e.id = local_id(&e.rev, e.at, nth);
        }
    }
    entries
}

pub fn diff_request(entry: &AuditEntry, newer: &[AuditEntry], ids: bool) -> Option<HistoryRequest> {
    if !ids && newer.iter().any(|n| n.rev == entry.rev) {
        return None;
    }
    Some(HistoryRequest {
        limit: Some(1),
        diff: (!ids).then(|| entry.rev.clone()),
        entry: ids.then_some(entry.id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::{self, json};

    fn entry(id: u64, rev: &str) -> AuditEntry {
        AuditEntry {
            id,
            rev: rev.into(),
            ..AuditEntry::default()
        }
    }

    fn at(rev: &str, at: u64) -> AuditEntry {
        AuditEntry {
            at,
            ..entry(0, rev)
        }
    }

    #[test]
    fn a_server_with_ids_is_asked_by_id_and_an_older_one_by_rev() {
        let e = entry(7, "b2s:aa");
        let new = serde_json::to_value(diff_request(&e, &[], true)).unwrap();
        assert_eq!(new, json!({"limit": 1, "entry": 7}));
        let old = serde_json::to_value(diff_request(&e, &[], false)).unwrap();
        assert_eq!(old, json!({"limit": 1, "diff": "b2s:aa"}));
        let repeated = [entry(9, "b2s:aa")];
        assert!(
            diff_request(&e, &repeated, false).is_none(),
            "an older server would diff the newer one"
        );
        assert!(diff_request(&e, &repeated, true).is_some());
    }

    #[test]
    fn entries_of_an_older_server_get_stable_ids_from_their_rev() {
        let old = vec![at("b2s:aa", 30), at("b2s:bb", 20), at("b2s:aa", 10)];
        let a = with_ids(old.clone());
        let b = with_ids(old.clone());
        assert_eq!(a, b, "the same entries get the same ids on every load");
        assert!(a.iter().all(|e| e.id != 0));
        assert_ne!(a[0].id, a[2].id, "A -> B -> A gives two rows");
        let mut later = vec![at("b2s:cc", 40)];
        later.extend(old);
        let c = with_ids(later);
        assert_eq!(&c[1..], &a[..], "a newer entry does not move the older ids");
        let same_second = with_ids(vec![at("b2s:aa", 5), at("b2s:aa", 5)]);
        assert_ne!(same_second[0].id, same_second[1].id);
        let kept = with_ids(vec![entry(5, "b2s:aa")]);
        assert_eq!(kept[0].id, 5);
    }

    #[test]
    fn the_feature_decides() {
        let w: Welcome = serde_json::from_value(json!({
            "server": {"version": "0.1.2"}, "protocol": 1,
            "you": {"visitor": "ops", "level": "read"},
            "features": ["history"], "limits": {"max_message": 1, "max_inflight": 1}, "schema_hash": "h"
        }))
        .unwrap();
        assert!(!has_ids(Some(&w)));
        assert!(!has_ids(None));
        let mut newer = w.clone();
        newer.features.push(HISTORY_IDS.into());
        assert!(has_ids(Some(&newer)));
    }
}
