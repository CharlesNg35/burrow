use burrow_admin_proto::status::{ClientStatus, Status};
use burrow_admin_proto::updates::{
    FIRST_REPORTING, Shown, UpdateState, UpdateStatus, hand_hint, platform_text, remote_text,
    run_text, state_text, version, version_text,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Quiet,
    Good,
    Busy,
    Offer,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientInfo {
    pub name: String,
    pub online: bool,
    pub local: bool,
    pub version: String,
    pub older: bool,
    pub system: Option<String>,
    pub run: Option<&'static str>,
    pub remote: Option<String>,
    pub available: Option<String>,
    pub blocked: Option<String>,
    pub update: Option<UpdateStatus>,
}

pub fn client<'a>(status: &'a Status, name: &str) -> Option<&'a ClientStatus> {
    status.clients.iter().find(|c| c.name == name)
}

pub fn info(c: &ClientStatus) -> ClientInfo {
    ClientInfo {
        name: c.name.clone(),
        online: c.connected || c.local,
        local: c.local,
        version: version_text(c),
        older: version(c) == Shown::Older,
        system: c.build.as_ref().map(platform_text),
        run: c.build.as_ref().map(run_text),
        remote: c.build.as_ref().map(remote_text),
        available: c.available.clone(),
        blocked: c.blocked.clone(),
        update: c.update.clone(),
    }
}

pub fn of(status: Option<&Status>, name: &str) -> Option<ClientInfo> {
    status.and_then(|s| client(s, name)).map(info)
}

impl ClientInfo {
    pub fn ready(&self) -> bool {
        self.available.is_some() && self.blocked.is_none()
    }

    pub fn tag(&self) -> Option<(String, Mood)> {
        match (&self.update, &self.available) {
            (Some(u), _) if !u.state.finished() => Some((state_text(u.state).into(), Mood::Busy)),
            (_, Some(v)) if self.blocked.is_some() => Some((format!("{v} available"), Mood::Quiet)),
            (_, Some(v)) => Some((format!("{v} available"), Mood::Offer)),
            _ => None,
        }
    }

    pub fn summary(&self) -> String {
        match (&self.system, self.run, &self.remote) {
            (Some(s), Some(r), Some(u)) => format!("{s} · {r} · remote updates {u}"),
            _ => String::new(),
        }
    }

    pub fn older_note(&self) -> Option<String> {
        self.older
            .then(|| format!("Older than {FIRST_REPORTING}: {}.", hand_hint()))
    }

    pub fn last_update(&self) -> Option<(String, Mood)> {
        let u = self.update.as_ref()?;
        let mut line = format!("{} → {}: {}", u.from, u.to, state_text(u.state));
        if !u.detail.is_empty() && u.state.finished() {
            line.push_str(&format!(" ({})", u.detail));
        }
        if !u.by.is_empty() {
            line.push_str(&format!(", asked by {}", u.by));
        }
        let mood = match u.state {
            UpdateState::Done => Mood::Good,
            s if s.failed() => Mood::Bad,
            _ => Mood::Busy,
        };
        Some((line, mood))
    }
}

pub fn ready_names(status: &Status) -> Vec<String> {
    status
        .clients
        .iter()
        .map(info)
        .filter(ClientInfo::ready)
        .map(|c| c.name)
        .collect()
}

pub fn rollout_line(st: &Status) -> Option<(String, Mood)> {
    let r = st.rollout.as_ref()?;
    let busy = usize::from(r.current.is_some());
    let left = r.clients.len().saturating_sub(r.done.len() + busy);
    if let Some(c) = &r.stopped_at {
        let why = client(st, c)
            .and_then(|x| x.update.as_ref())
            .map(|u| format!(" ({}: {})", state_text(u.state), u.detail))
            .unwrap_or_default();
        return Some((
            format!(
                "Stopped updating to {} at {c}{why}; {} updated",
                r.to,
                r.done.len()
            ),
            Mood::Bad,
        ));
    }
    match (&r.current, r.finished) {
        (Some(c), _) => Some((
            format!(
                "Updating to {}: {c} now, {} done, {left} waiting",
                r.to,
                r.done.len()
            ),
            Mood::Busy,
        )),
        (None, true) => Some((
            format!("Updated {} to {}", r.done.join(", "), r.to),
            Mood::Good,
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::updates::{ClientBuild, RemoteUpdates, RolloutStatus, RunMode};

    fn status() -> Status {
        let build = |v: &str| ClientBuild {
            version: v.into(),
            os: "linux".into(),
            arch: "arm64".into(),
            run: RunMode::Service,
            remote: RemoteUpdates::On,
            why_not: None,
        };
        Status {
            clients: vec![
                ClientStatus {
                    name: "nas".into(),
                    connected: true,
                    build: Some(build("0.2.1")),
                    available: Some("0.2.2".into()),
                    ..ClientStatus::default()
                },
                ClientStatus {
                    name: "pi".into(),
                    connected: true,
                    build: Some(build("0.2.1")),
                    available: Some("0.2.2".into()),
                    blocked: Some("remote updates are off".into()),
                    ..ClientStatus::default()
                },
                ClientStatus {
                    name: "old".into(),
                    connected: true,
                    ..ClientStatus::default()
                },
            ],
            ..Status::default()
        }
    }

    #[test]
    fn a_client_reads_as_its_version_system_and_what_it_can_take() {
        let st = status();
        let nas = of(Some(&st), "nas").unwrap();
        assert_eq!(nas.version, "0.2.1");
        assert_eq!(
            nas.summary(),
            "linux/arm64 · service · remote updates allowed"
        );
        assert_eq!(nas.tag(), Some(("0.2.2 available".into(), Mood::Offer)));
        assert!(nas.ready());
        let pi = of(Some(&st), "pi").unwrap();
        assert!(!pi.ready());
        assert_eq!(pi.tag(), Some(("0.2.2 available".into(), Mood::Quiet)));
        let old = of(Some(&st), "old").unwrap();
        assert!(old.older && old.summary().is_empty());
        assert!(old.older_note().unwrap().contains("sudo burrow update"));
        assert_eq!(of(None, "nas"), None);
        assert_eq!(ready_names(&st), ["nas"]);
    }

    #[test]
    fn an_update_in_progress_and_its_end_read_plainly() {
        let mut st = status();
        st.clients[0].update = Some(UpdateStatus {
            id: 1,
            from: "0.2.1".into(),
            to: "0.2.2".into(),
            state: UpdateState::Downloading,
            detail: "x".into(),
            by: "ops (203.0.113.7)".into(),
            secs_ago: 2,
        });
        let nas = of(Some(&st), "nas").unwrap();
        assert_eq!(nas.tag().unwrap().1, Mood::Busy);
        assert_eq!(
            nas.last_update().unwrap().0,
            "0.2.1 → 0.2.2: downloading and checking, asked by ops (203.0.113.7)"
        );
        st.clients[0].update.as_mut().unwrap().state = UpdateState::RolledBack;
        st.rollout = Some(RolloutStatus {
            to: "0.2.2".into(),
            clients: vec!["nas".into(), "pi".into()],
            stopped_at: Some("nas".into()),
            finished: true,
            ..RolloutStatus::default()
        });
        let (line, mood) = rollout_line(&st).unwrap();
        assert_eq!(
            line,
            "Stopped updating to 0.2.2 at nas (rolled back: x); 0 updated"
        );
        assert_eq!(mood, Mood::Bad);
        assert_eq!(
            of(Some(&st), "nas").unwrap().last_update().unwrap().1,
            Mood::Bad
        );
    }
}
