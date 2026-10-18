use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use super::{ClientSession, Shared};
use crate::config::settings::ServerSettings;
use crate::forward::ForwardError;
use crate::protocol::{Status, TOKEN_LEN};

const FIRST_WAIT: Duration = Duration::from_secs(10);
const LONGEST_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seen {
    Clear,
    Aside,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trouble {
    Refused,
    Failed,
}

#[derive(Debug, Clone, Copy)]
enum Mark {
    Refused { session: [u8; TOKEN_LEN] },
    Failed { until: Instant, wait: Duration },
}

type Key = (String, String, String);

#[derive(Debug, PartialEq, Eq)]
enum Change {
    Nothing,
    Refused,
    SetAside(Duration),
    Recovered,
}

#[derive(Default)]
pub(crate) struct Standing(HashMap<Key, Mark>);

fn key(service: &str, client: &str, target: &str) -> Key {
    (service.to_string(), client.to_string(), target.to_string())
}

impl Standing {
    fn of(
        &self,
        service: &str,
        client: &str,
        target: &str,
        session: &[u8; TOKEN_LEN],
        now: Instant,
    ) -> Seen {
        match self.0.get(&key(service, client, target)) {
            Some(Mark::Refused { session: s }) if s == session => Seen::Refused,
            Some(Mark::Failed { until, .. }) if now < *until => Seen::Aside,
            _ => Seen::Clear,
        }
    }

    fn marked(&self, service: &str, client: &str, session: &[u8; TOKEN_LEN], now: Instant) -> Seen {
        let mut seen = Seen::Clear;
        for ((s, c, _), mark) in &self.0 {
            if s != service || c != client {
                continue;
            }
            match mark {
                Mark::Refused { session: m } if m == session => return Seen::Refused,
                Mark::Failed { until, .. } if now < *until => seen = Seen::Aside,
                _ => {}
            }
        }
        seen
    }

    fn accepted(&mut self, service: &str, client: &str, target: &str) -> Change {
        match self.0.remove(&key(service, client, target)) {
            Some(Mark::Failed { .. }) => Change::Recovered,
            _ => Change::Nothing,
        }
    }

    fn troubled(
        &mut self,
        service: &str,
        client: &str,
        target: &str,
        session: &[u8; TOKEN_LEN],
        trouble: Trouble,
        now: Instant,
    ) -> Change {
        let before = self.marked(service, client, session, now);
        let entry = self.0.entry(key(service, client, target));
        let previous = match &entry {
            std::collections::hash_map::Entry::Occupied(e) => Some(*e.get()),
            std::collections::hash_map::Entry::Vacant(_) => None,
        };
        match trouble {
            Trouble::Refused => {
                entry.insert_entry(Mark::Refused { session: *session });
                if before == Seen::Refused {
                    Change::Nothing
                } else {
                    Change::Refused
                }
            }
            Trouble::Failed => {
                let wait = match previous {
                    Some(Mark::Failed { until, .. }) if now < until => return Change::Nothing,
                    Some(Mark::Failed { wait, .. }) => (wait * 2).min(LONGEST_WAIT),
                    _ => FIRST_WAIT,
                };
                entry.insert_entry(Mark::Failed {
                    until: now + wait,
                    wait,
                });
                if before == Seen::Clear {
                    Change::SetAside(wait)
                } else {
                    Change::Nothing
                }
            }
        }
    }

    fn retain(&mut self, mut keep: impl FnMut(&str, &str, &str) -> bool) {
        self.0.retain(|(s, c, t), _| keep(s, c, t));
    }
}

fn trouble_of(error: &ForwardError) -> Option<(Trouble, &'static str)> {
    match error {
        ForwardError::Status(Status::Denied | Status::Invalid) => Some((Trouble::Refused, "")),
        ForwardError::Status(Status::Unreachable) => Some((Trouble::Failed, "cannot connect")),
        ForwardError::Status(Status::Unresolved) => Some((Trouble::Failed, "cannot resolve it")),
        ForwardError::Status(Status::Busy) => Some((Trouble::Failed, "too many connections open")),
        ForwardError::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => {
            Some((Trouble::Failed, "no answer in time"))
        }
        ForwardError::Status(_) | ForwardError::Protocol | ForwardError::Io(_) => None,
    }
}

impl Shared {
    pub(super) fn seen(&self, service: &str, session: &ClientSession, target: &str) -> Seen {
        self.standing.lock().expect("standing lock").of(
            service,
            &session.name,
            target,
            &session.token,
            Instant::now(),
        )
    }

    pub(super) fn seen_any(&self, service: &str, session: &ClientSession) -> Seen {
        self.standing.lock().expect("standing lock").marked(
            service,
            &session.name,
            &session.token,
            Instant::now(),
        )
    }

    pub(super) fn accepted(&self, service: &str, client: &str, target: &str) {
        let change = self
            .standing
            .lock()
            .expect("standing lock")
            .accepted(service, client, target);
        if change == Change::Recovered {
            info!("service \"{service}\": client \"{client}\" reaches {target} again");
        }
    }

    pub(super) fn troubled(
        &self,
        service: &str,
        session: &ClientSession,
        target: &str,
        error: &ForwardError,
    ) {
        let Some((trouble, why)) = trouble_of(error) else {
            return;
        };
        if session.is_closed() {
            return;
        }
        let client = &session.name;
        let change = self.standing.lock().expect("standing lock").troubled(
            service,
            client,
            target,
            &session.token,
            trouble,
            Instant::now(),
        );
        match change {
            Change::Refused if matches!(error, ForwardError::Status(Status::Invalid)) => warn!(
                "service \"{service}\": client \"{client}\" rejected the target {target} as invalid; new connections use the other clients until it reconnects"
            ),
            Change::Refused => warn!(
                "service \"{service}\": client \"{client}\" refused {target} because it is not in its allow list; new connections use the other clients until it reconnects (add it to \"allow\" in the config of client \"{client}\")"
            ),
            Change::SetAside(wait) => warn!(
                "service \"{service}\": client \"{client}\" did not get through to {target} ({why}); new connections try the other clients first for {}s",
                wait.as_secs()
            ),
            Change::Nothing | Change::Recovered => {}
        }
    }

    pub(super) fn prune_standing(&self, settings: &ServerSettings) {
        self.standing
            .lock()
            .expect("standing lock")
            .retain(|service, client, target| {
                settings.services.get(service).is_some_and(|svc| {
                    svc.clients.len() > 1
                        && svc.clients.iter().any(|c| c == client)
                        && (0..svc.ports.max(1)).any(|i| svc.target_at(i) == target)
                })
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: [u8; TOKEN_LEN] = [1; TOKEN_LEN];
    const TWO: [u8; TOKEN_LEN] = [2; TOKEN_LEN];
    const T: &str = "192.168.50.22:22";

    #[test]
    fn a_refusal_lasts_until_the_client_reconnects() {
        let mut s = Standing::default();
        let now = Instant::now();
        assert_eq!(
            s.troubled("ssh", "b", T, &ONE, Trouble::Refused, now),
            Change::Refused
        );
        assert_eq!(
            s.of("ssh", "b", T, &ONE, now + LONGEST_WAIT * 10),
            Seen::Refused
        );
        assert_eq!(s.of("ssh", "b", T, &TWO, now), Seen::Clear);
        assert_eq!(s.of("ssh", "a", T, &ONE, now), Seen::Clear);
        assert_eq!(s.of("ssh", "b", "10.0.0.1:22", &ONE, now), Seen::Clear);
        assert_eq!(
            s.troubled("ssh", "b", "192.168.50.23:22", &ONE, Trouble::Refused, now),
            Change::Nothing,
            "one warning per client and service"
        );
        assert_eq!(
            s.troubled("ssh", "b", T, &TWO, Trouble::Refused, now),
            Change::Refused,
            "a new session that refuses again is reported again"
        );
    }

    #[test]
    fn a_failing_client_is_set_aside_for_longer_each_time_until_it_works() {
        let mut s = Standing::default();
        let now = Instant::now();
        assert_eq!(
            s.troubled("web", "a", T, &ONE, Trouble::Failed, now),
            Change::SetAside(FIRST_WAIT)
        );
        assert_eq!(s.of("web", "a", T, &ONE, now), Seen::Aside);
        assert_eq!(
            s.of("web", "a", T, &TWO, now),
            Seen::Aside,
            "a down target stays down across a reconnect"
        );
        assert_eq!(
            s.troubled(
                "web",
                "a",
                T,
                &ONE,
                Trouble::Failed,
                now + Duration::from_secs(1)
            ),
            Change::Nothing,
            "failures while set aside do not extend the wait"
        );
        let later = now + FIRST_WAIT;
        assert_eq!(s.of("web", "a", T, &ONE, later), Seen::Clear);
        assert_eq!(
            s.troubled("web", "a", T, &ONE, Trouble::Failed, later),
            Change::SetAside(FIRST_WAIT * 2)
        );
        let mut at = later;
        for _ in 0..5 {
            let wait = match s.0[&key("web", "a", T)] {
                Mark::Failed { wait, .. } => wait,
                Mark::Refused { .. } => unreachable!(),
            };
            at += wait;
            s.troubled("web", "a", T, &ONE, Trouble::Failed, at);
        }
        assert!(matches!(
            s.0[&key("web", "a", T)],
            Mark::Failed { wait, .. } if wait == LONGEST_WAIT
        ));
        assert_eq!(s.accepted("web", "a", T), Change::Recovered);
        assert_eq!(s.of("web", "a", T, &ONE, at), Seen::Clear);
        assert_eq!(
            s.troubled("web", "a", T, &ONE, Trouble::Failed, at),
            Change::SetAside(FIRST_WAIT),
            "a success starts over"
        );
        assert_eq!(s.accepted("web", "b", T), Change::Nothing);
    }

    #[test]
    fn a_target_failing_on_one_port_does_not_set_aside_the_others() {
        let mut s = Standing::default();
        let now = Instant::now();
        s.troubled("range", "a", "10.0.0.1:5000", &ONE, Trouble::Failed, now);
        assert_eq!(s.of("range", "a", "10.0.0.1:5001", &ONE, now), Seen::Clear);
        assert_eq!(s.marked("range", "a", &ONE, now), Seen::Aside);
        assert_eq!(
            s.troubled("range", "a", "10.0.0.1:5001", &ONE, Trouble::Failed, now),
            Change::Nothing,
            "one warning per client and service"
        );
        s.troubled("range", "a", "10.0.0.1:5002", &ONE, Trouble::Refused, now);
        assert_eq!(s.marked("range", "a", &ONE, now), Seen::Refused);
        assert_eq!(s.marked("range", "a", &TWO, now), Seen::Aside);
    }

    #[test]
    fn only_what_the_client_says_about_the_target_counts() {
        let kind = |e: ForwardError| trouble_of(&e).map(|t| t.0);
        assert_eq!(
            kind(ForwardError::Status(Status::Denied)),
            Some(Trouble::Refused)
        );
        assert_eq!(
            kind(ForwardError::Status(Status::Invalid)),
            Some(Trouble::Refused)
        );
        assert_eq!(
            kind(ForwardError::Status(Status::Unreachable)),
            Some(Trouble::Failed)
        );
        assert_eq!(
            kind(ForwardError::Status(Status::Unresolved)),
            Some(Trouble::Failed)
        );
        assert_eq!(kind(ForwardError::Protocol), None);
        assert_eq!(
            kind(ForwardError::Status(Status::Busy)),
            Some(Trouble::Failed),
            "a client at its connection limit is set aside like one that cannot connect"
        );
        assert_eq!(
            kind(ForwardError::Io(std::io::ErrorKind::ConnectionReset.into())),
            None,
            "a channel cut by a disconnect says nothing about the target"
        );
        assert_eq!(
            kind(ForwardError::Io(std::io::ErrorKind::TimedOut.into())),
            Some(Trouble::Failed)
        );
    }
}
