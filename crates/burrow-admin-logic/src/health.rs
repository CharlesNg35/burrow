use burrow_admin_proto::status::{EntryStatus, HealthStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Down,
    Partial,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Count {
    pub up: usize,
    pub of: usize,
    pub checked: bool,
}

impl Count {
    pub fn level(self) -> Level {
        if self.up == 0 {
            Level::Down
        } else if self.up < self.of {
            Level::Partial
        } else {
            Level::Up
        }
    }

    pub fn fraction(self) -> String {
        format!("{} of {} up", self.up, self.of)
    }

    pub fn label(self) -> String {
        match self.level() {
            Level::Down => "Down".into(),
            Level::Partial => self.fraction(),
            Level::Up if self.checked => "Healthy".into(),
            Level::Up => "Up".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryHealth {
    Up,
    Down(Option<String>),
    Offline,
    NoCheck,
}

pub fn entries(listed: &[EntryStatus], names: Vec<String>) -> Vec<(String, Option<String>)> {
    if listed.is_empty() {
        names.into_iter().map(|n| (n, None)).collect()
    } else {
        listed
            .iter()
            .map(|e| (e.client.clone(), e.target.clone()))
            .collect()
    }
}

fn check<'a>(
    health: &'a [HealthStatus],
    client: &str,
    target: Option<&str>,
) -> Option<&'a HealthStatus> {
    health
        .iter()
        .find(|h| h.client == client && h.target.as_deref() == target)
}

pub fn entry(
    health: &[HealthStatus],
    client: &str,
    target: Option<&str>,
    online: bool,
) -> EntryHealth {
    match (online, check(health, client, target)) {
        (false, _) => EntryHealth::Offline,
        (true, Some(h)) if h.up => EntryHealth::Up,
        (true, Some(h)) => EntryHealth::Down(h.reason.clone()),
        (true, None) => EntryHealth::NoCheck,
    }
}

pub fn count(
    entries: &[(String, Option<String>)],
    health: &[HealthStatus],
    online: impl Fn(&str) -> bool,
) -> Count {
    let up = entries
        .iter()
        .filter(|(c, t)| {
            matches!(
                entry(health, c, t.as_deref(), online(c)),
                EntryHealth::Up | EntryHealth::NoCheck
            )
        })
        .count();
    Count {
        up,
        of: entries.len(),
        checked: !health.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(client: &str, up: bool) -> HealthStatus {
        HealthStatus {
            client: client.into(),
            target: None,
            up,
            reason: (!up).then(|| "cannot connect".into()),
        }
    }

    fn names(n: &[&str]) -> Vec<(String, Option<String>)> {
        entries(&[], n.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn an_offline_client_and_a_failing_check_both_count_as_not_up() {
        let all = names(&["a", "b", "c", "d"]);
        let health = [h("a", true), h("b", true), h("c", false)];
        let c = count(&all, &health, |n| n != "d");
        assert_eq!(
            c,
            Count {
                up: 2,
                of: 4,
                checked: true
            }
        );
        assert_eq!(c.label(), "2 of 4 up");
        assert_eq!(c.fraction(), "2 of 4 up");
        assert_eq!(c.level(), Level::Partial);
    }

    #[test]
    fn labels_say_down_healthy_or_up() {
        let all = names(&["a", "b"]);
        assert_eq!(count(&all, &[], |_| false).label(), "Down");
        assert_eq!(count(&all, &[], |_| true).label(), "Up");
        assert_eq!(
            count(&all, &[h("a", true), h("b", true)], |_| true).label(),
            "Healthy"
        );
    }

    #[test]
    fn an_offline_client_is_offline_not_unchecked() {
        let health = [h("a", true)];
        assert_eq!(entry(&health, "b", None, false), EntryHealth::Offline);
        assert_eq!(entry(&health, "a", None, false), EntryHealth::Offline);
        assert_eq!(entry(&health, "b", None, true), EntryHealth::NoCheck);
        assert_eq!(
            entry(&[h("a", false)], "a", None, true),
            EntryHealth::Down(Some("cannot connect".into()))
        );
    }

    #[test]
    fn listed_entries_keep_their_own_targets() {
        let listed = [EntryStatus {
            client: "a".into(),
            target: Some("10.0.0.2:80".into()),
            open: 0,
        }];
        assert_eq!(
            entries(&listed, vec!["ignored".into()]),
            vec![("a".to_string(), Some("10.0.0.2:80".to_string()))]
        );
    }
}
