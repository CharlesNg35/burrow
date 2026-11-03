use burrow_admin_proto::status::Status;
use burrow_admin_proto::updates::{RemoteUpdates, UpdateState, UpdateStatus, job_text, state_text};

use crate::presence::Mood;
use crate::version::server_is_older;

pub const UPDATING: &str = "updating, the server restarts…";
pub const RESULT_SHOWN_SECS: u64 = 24 * 3600;
pub const DONE_SHOWN_SECS: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerFigure {
    pub to: Option<String>,
    pub state: Option<(String, Mood)>,
    pub available: Option<(String, Mood)>,
    pub why_not: Option<String>,
    pub can_ask: bool,
    pub detail: String,
}

impl ServerFigure {
    pub fn says_something(&self) -> bool {
        self.state.is_some() || self.available.is_some()
    }
}

fn shown_for(state: UpdateState) -> u64 {
    match state {
        UpdateState::Done => DONE_SHOWN_SECS,
        _ => RESULT_SHOWN_SECS,
    }
}

fn result(u: &UpdateStatus) -> (String, Mood) {
    match u.state {
        UpdateState::Done => (format!("updated {} → {}", u.from, u.to), Mood::Good),
        s if u.detail.is_empty() => (state_text(s).to_string(), Mood::Bad),
        s => (format!("{}: {}", state_text(s), u.detail), Mood::Bad),
    }
}

pub fn figure(st: &Status) -> Option<ServerFigure> {
    let own = st.server_update.as_ref()?;
    let latest = st.latest.as_ref().and_then(|l| l.version.clone());
    let on = own.remote == RemoteUpdates::On;
    let running = own.running();
    let to = latest.filter(|l| server_is_older(&st.version, l) && !running);
    let recent = own
        .last
        .as_ref()
        .filter(|u| running || u.secs_ago < shown_for(u.state));
    let state = match recent {
        Some(_) if running => Some((UPDATING.to_string(), Mood::Busy)),
        Some(u) => Some(result(u)),
        None => None,
    };
    let detail = recent
        .map(|u| {
            let mut line = job_text(u);
            if !u.by.is_empty() {
                line.push_str(&format!(", asked by {}", u.by));
            }
            line
        })
        .unwrap_or_default();
    Some(ServerFigure {
        can_ask: on && to.is_some(),
        available: to.as_ref().map(|v| {
            let mood = if on { Mood::Offer } else { Mood::Quiet };
            (format!("{v} available"), mood)
        }),
        to,
        state,
        why_not: own.why_not.clone().filter(|_| !on),
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::updates::{LatestRelease, ServerUpdate};

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

    fn job(state: UpdateState, detail: &str, secs_ago: u64) -> UpdateStatus {
        UpdateStatus {
            from: "0.2.2".into(),
            to: "0.2.3".into(),
            state,
            detail: detail.into(),
            by: "ops (203.0.113.7)".into(),
            secs_ago,
            ..UpdateStatus::default()
        }
    }

    #[test]
    fn nothing_to_say_without_a_newer_release_or_a_recent_update() {
        for remote in [
            RemoteUpdates::On,
            RemoteUpdates::Off,
            RemoteUpdates::Blocked,
        ] {
            let f = figure(&status(remote, "0.2.2", None)).unwrap();
            assert!(!f.says_something() && !f.can_ask, "{remote:?}");
        }
        let old = job(UpdateState::Done, "", DONE_SHOWN_SECS);
        let f = figure(&status(RemoteUpdates::On, "0.2.2", Some(old))).unwrap();
        assert!(
            !f.says_something(),
            "an hour after an update the header is as before"
        );
        let failed = job(UpdateState::RolledBack, "x", DONE_SHOWN_SECS);
        let f = figure(&status(RemoteUpdates::On, "0.2.2", Some(failed))).unwrap();
        assert!(f.says_something(), "a roll back stays for a day");
        let older = job(UpdateState::RolledBack, "x", RESULT_SHOWN_SECS);
        let f = figure(&status(RemoteUpdates::On, "0.2.2", Some(older))).unwrap();
        assert!(!f.says_something());
        assert_eq!(
            figure(&Status::default()),
            None,
            "an older server says nothing"
        );
    }

    #[test]
    fn a_newer_release_is_offered_only_when_remote_updates_are_on() {
        let on = figure(&status(RemoteUpdates::On, "0.2.3", None)).unwrap();
        assert!(on.can_ask);
        assert_eq!(on.available, Some(("0.2.3 available".into(), Mood::Offer)));
        assert_eq!(on.why_not, None);
        let off = figure(&status(RemoteUpdates::Off, "0.2.3", None)).unwrap();
        assert!(!off.can_ask);
        assert_eq!(off.available.unwrap().1, Mood::Quiet);
        assert!(
            off.why_not.unwrap().contains("update.remote: true"),
            "the tag's tooltip"
        );
    }

    #[test]
    fn a_running_update_and_its_result_take_the_same_line() {
        let busy = figure(&status(
            RemoteUpdates::On,
            "0.2.3",
            Some(job(UpdateState::Restarting, "", 3)),
        ))
        .unwrap();
        assert_eq!(busy.state, Some((UPDATING.into(), Mood::Busy)));
        assert!(!busy.can_ask && busy.available.is_none());
        let done = figure(&status(
            RemoteUpdates::On,
            "0.2.3",
            Some(job(UpdateState::Done, "1 of 1 clients connected again", 9)),
        ))
        .unwrap();
        assert_eq!(
            done.state,
            Some(("updated 0.2.2 → 0.2.3".into(), Mood::Good))
        );
        assert!(
            done.detail.ends_with("asked by ops (203.0.113.7)"),
            "{}",
            done.detail
        );
        let back = figure(&status(
            RemoteUpdates::On,
            "0.2.3",
            Some(job(UpdateState::RolledBack, "0.2.3 is not running", 9)),
        ))
        .unwrap();
        assert_eq!(
            back.state,
            Some(("rolled back: 0.2.3 is not running".into(), Mood::Bad))
        );
        assert!(back.can_ask, "it may be asked again");
    }
}
