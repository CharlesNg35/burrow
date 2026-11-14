use std::collections::BTreeSet;

use burrow_admin_proto::serde_json::Value;
use burrow_config::{Entry, ServiceEntry};

use crate::addr;

pub const TRIES: usize = 20;

pub fn is_service_bind(path: &[String]) -> bool {
    matches!(path, [section, _, field] if section == ServiceEntry::section() && field == ServiceEntry::BIND.key())
}

pub fn ports_of(v: Option<&Value>) -> Vec<u16> {
    let p = addr::split(v);
    let Ok(from) = p.from.trim().parse::<u16>() else {
        return Vec::new();
    };
    let to = p.to.trim().parse::<u16>().unwrap_or(from).max(from);
    (from..=to).collect()
}

pub fn draft_ports(draft: &Value, except: &str) -> BTreeSet<u16> {
    draft
        .get(ServiceEntry::section())
        .and_then(Value::as_object)
        .map(|services| {
            services
                .iter()
                .filter(|(name, _)| name.as_str() != except)
                .flat_map(|(_, s)| ports_of(ServiceEntry::BIND.get(s)))
                .collect()
        })
        .unwrap_or_default()
}

pub fn wanted(range_on: bool, target: Option<u32>, own: Option<u32>) -> Option<u16> {
    if !range_on {
        return Some(1);
    }
    [target, own]
        .into_iter()
        .flatten()
        .find(|n| *n > 1)
        .and_then(|n| u16::try_from(n).ok())
}

pub fn near_after(current: &str, count: u16) -> Option<u32> {
    current
        .trim()
        .parse::<u32>()
        .ok()
        .map(|p| p + u32::from(count))
}

pub fn clash(start: u16, count: u16, used: &BTreeSet<u16>) -> Option<u32> {
    let end = u32::from(start) + u32::from(count);
    (u32::from(start)..end)
        .filter_map(|p| u16::try_from(p).ok())
        .filter(|p| used.contains(p))
        .max()
        .map(|p| u32::from(p) + 1)
}

pub fn tip(count: u16) -> String {
    if count > 1 {
        format!("Propose {count} free ports in a row")
    } else {
        "Propose a free port".into()
    }
}

pub const NEEDS_SIZE: &str = "Set the target's port range first";

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;

    fn path(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_a_service_bind_gets_a_proposal() {
        assert!(is_service_bind(&path(&["services", "web", "bind"])));
        assert!(!is_service_bind(&path(&["services", "web", "target"])));
        assert!(!is_service_bind(&path(&["visitors", "a", "bind"])));
        assert!(!is_service_bind(&path(&["listen"])));
    }

    #[test]
    fn ports_in_the_draft_skip_the_edited_service() {
        let draft = json!({"services": {
            "web": {"bind": "0.0.0.0:7841"},
            "game": {"bind": "0.0.0.0:7850-7852"},
            "ssh": {"target": 22},
            "me": {"bind": 7843}
        }});
        let used: Vec<u16> = draft_ports(&draft, "me").into_iter().collect();
        assert_eq!(used, [7841, 7850, 7851, 7852]);
    }

    #[test]
    fn a_range_proposes_the_size_the_target_needs() {
        assert_eq!(wanted(false, Some(3), None), Some(1));
        assert_eq!(wanted(true, Some(3), Some(2)), Some(3));
        assert_eq!(wanted(true, Some(1), Some(4)), Some(4));
        assert_eq!(wanted(true, Some(1), None), None);
        assert_eq!(wanted(true, None, Some(1)), None);
    }

    #[test]
    fn a_proposal_clashing_with_the_draft_moves_past_it() {
        let used: BTreeSet<u16> = [7841, 7843].into();
        assert_eq!(clash(7840, 1, &used), None);
        assert_eq!(clash(7841, 1, &used), Some(7842));
        assert_eq!(clash(7840, 4, &used), Some(7844));
        assert_eq!(clash(65535, 1, &BTreeSet::from([65535])), Some(65536));
    }

    #[test]
    fn the_next_click_starts_after_the_current_proposal() {
        assert_eq!(near_after("7841", 1), Some(7842));
        assert_eq!(near_after("7841", 3), Some(7844));
        assert_eq!(near_after("", 1), None);
        assert_eq!(tip(1), "Propose a free port");
        assert_eq!(tip(3), "Propose 3 free ports in a row");
    }
}
