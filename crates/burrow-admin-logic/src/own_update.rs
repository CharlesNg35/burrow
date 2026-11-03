use burrow_admin_proto::status::Status;
use burrow_admin_proto::updates::{RemoteUpdates, UpdateState, job_text};

use crate::presence::Mood;
use crate::version::server_is_older;

pub const UPDATING: &str = "updating, the server restarts";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerFigure {
    pub version: String,
    pub to: Option<String>,
    pub tag: Option<(String, Mood)>,
    pub remote: &'static str,
    pub why_not: Option<String>,
    pub can_ask: bool,
    pub last: Option<(String, Mood)>,
}

pub fn figure(st: &Status) -> Option<ServerFigure> {
    let own = st.server_update.as_ref()?;
    let latest = st.latest.as_ref().and_then(|l| l.version.clone());
    let to = latest.filter(|l| server_is_older(&st.version, l));
    let on = own.remote == RemoteUpdates::On;
    let running = own.running();
    let tag = match (&to, running) {
        (_, true) => Some((UPDATING.to_string(), Mood::Busy)),
        (Some(v), false) if on => Some((format!("{v} available"), Mood::Offer)),
        (Some(v), false) => Some((format!("{v} available"), Mood::Quiet)),
        (None, false) => None,
    };
    let last = own.last.as_ref().filter(|u| u.state.finished()).map(|u| {
        let mut line = job_text(u);
        if !u.by.is_empty() {
            line.push_str(&format!(", asked by {}", u.by));
        }
        let mood = match u.state {
            UpdateState::Done => Mood::Good,
            _ => Mood::Bad,
        };
        (line, mood)
    });
    Some(ServerFigure {
        version: st.version.clone(),
        can_ask: on && !running && to.is_some(),
        to,
        tag,
        remote: match own.remote {
            RemoteUpdates::On => "updates from burrow admin on",
            RemoteUpdates::Off => "updates from burrow admin off",
            _ => "updates from burrow admin not possible",
        },
        why_not: own.why_not.clone().filter(|_| !on),
        last,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::updates::{LatestRelease, ServerUpdate, UpdateStatus};

    fn status(remote: RemoteUpdates, latest: &str, last: Option<UpdateStatus>) -> Status {
        Status {
            version: "0.2.2".into(),
            latest: Some(LatestRelease {
                version: Some(latest.into()),
                ..LatestRelease::default()
            }),
            server_update: Some(ServerUpdate {
                remote,
                why_not: (remote != RemoteUpdates::On)
                    .then(|| "turn them on with update.remote: true".into()),
                last,
                ..ServerUpdate::default()
            }),
            ..Status::default()
        }
    }

    #[test]
    fn a_newer_release_is_offered_only_when_remote_updates_are_on() {
        let on = figure(&status(RemoteUpdates::On, "0.2.3", None)).unwrap();
        assert!(on.can_ask);
        assert_eq!(on.to.as_deref(), Some("0.2.3"));
        assert_eq!(on.tag, Some(("0.2.3 available".into(), Mood::Offer)));
        assert_eq!(on.why_not, None);
        let off = figure(&status(RemoteUpdates::Off, "0.2.3", None)).unwrap();
        assert!(!off.can_ask);
        assert_eq!(off.tag.unwrap().1, Mood::Quiet);
        assert!(off.why_not.unwrap().contains("update.remote: true"));
        assert_eq!(off.remote, "updates from burrow admin off");
        let same = figure(&status(RemoteUpdates::On, "0.2.2", None)).unwrap();
        assert!(!same.can_ask && same.tag.is_none() && same.to.is_none());
        assert_eq!(
            figure(&Status::default()),
            None,
            "an older server says nothing"
        );
    }

    #[test]
    fn a_running_update_shows_the_restart_and_a_finished_one_its_result() {
        let mut u = UpdateStatus {
            from: "0.2.2".into(),
            to: "0.2.3".into(),
            state: UpdateState::Restarting,
            by: "ops (203.0.113.7)".into(),
            ..UpdateStatus::default()
        };
        let busy = figure(&status(RemoteUpdates::On, "0.2.3", Some(u.clone()))).unwrap();
        assert_eq!(busy.tag, Some((UPDATING.into(), Mood::Busy)));
        assert!(!busy.can_ask && busy.last.is_none());
        u.state = UpdateState::RolledBack;
        u.detail = "0.2.3 is not running".into();
        let back = figure(&status(RemoteUpdates::On, "0.2.3", Some(u))).unwrap();
        let (line, mood) = back.last.unwrap();
        assert_eq!(mood, Mood::Bad);
        assert!(line.contains("rolled back: 0.2.3 is not running"), "{line}");
        assert!(line.ends_with("asked by ops (203.0.113.7)"), "{line}");
    }
}
