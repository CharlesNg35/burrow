use std::net::{IpAddr, SocketAddr};

use burrow_admin_proto::status::{ClientStatus, OFFLINE, ONLINE, Status, away};
use burrow_admin_proto::updates::{
    FIRST_REPORTING, RemoteUpdates, Shown, UpdateState, UpdateStatus, hand_hint, platform_text,
    remote_text, run_text, state_text, version, version_text,
};

use crate::wording::THIS_SERVER;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Here,
    Online,
    Offline,
    Unknown,
}

impl Presence {
    pub fn up(self) -> bool {
        matches!(self, Presence::Here | Presence::Online)
    }

    pub fn word(self) -> &'static str {
        match self {
            Presence::Here => THIS_SERVER,
            Presence::Online => ONLINE,
            Presence::Offline => OFFLINE,
            Presence::Unknown => "",
        }
    }
}

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
    pub from: Option<String>,
    pub version: String,
    pub older: bool,
    pub system: Option<String>,
    pub run: Option<&'static str>,
    pub remote: Option<String>,
    pub allows: bool,
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
        from: c.from.as_deref().map(from_ip),
        version: version_text(c),
        older: version(c) == Shown::Older,
        system: c.build.as_ref().map(platform_text),
        run: c.build.as_ref().map(run_text),
        remote: c.build.as_ref().map(remote_text),
        allows: c
            .build
            .as_ref()
            .is_some_and(|b| b.remote == RemoteUpdates::On),
        available: c.available.clone(),
        blocked: c.blocked.clone(),
        update: c.update.clone().map(|u| UpdateStatus { secs_ago: 0, ..u }),
    }
}

pub fn of(status: Option<&Status>, name: &str) -> Option<ClientInfo> {
    status.and_then(|s| client(s, name)).map(info)
}

pub fn presence(status: Option<&Status>, reserved: &[String], name: &str) -> Presence {
    if reserved.iter().any(|r| r == name) {
        return Presence::Here;
    }
    match status {
        None => Presence::Unknown,
        Some(s) => match client(s, name) {
            Some(c) if c.local => Presence::Here,
            Some(c) if c.connected => Presence::Online,
            _ => Presence::Offline,
        },
    }
}

pub fn online(status: Option<&Status>, reserved: &[String], name: &str) -> bool {
    presence(status, reserved, name).up()
}

pub fn from_ip(from: &str) -> String {
    let from = from.trim();
    if let Ok(a) = from.parse::<SocketAddr>() {
        return a.ip().to_canonical().to_string();
    }
    let bare = from.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>()
        .map_or_else(|_| from.to_string(), |ip| ip.to_canonical().to_string())
}

pub fn detail(status: Option<&Status>, reserved: &[String], name: &str) -> String {
    match (
        presence(status, reserved, name),
        status.and_then(|s| client(s, name)),
    ) {
        (Presence::Online, Some(c)) => c.from.as_deref().map_or_else(
            || ONLINE.to_string(),
            |f| format!("{ONLINE} · {}", from_ip(f)),
        ),
        (p, _) => p.word().to_string(),
    }
}

pub fn away_text(status: &Status, name: &str, added_secs_ago: Option<u64>) -> String {
    let left = client(status, name).and_then(|c| c.left_secs_ago);
    away(left, status.uptime_secs, added_secs_ago)
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

    pub fn system_short(&self) -> String {
        match (&self.system, self.run) {
            (Some(s), Some(r)) => format!("{s} · {r}"),
            _ => String::new(),
        }
    }

    pub fn summary(&self) -> String {
        match &self.remote {
            Some(u) => format!("{} · remote updates {u}", self.system_short()),
            None => String::new(),
        }
    }

    pub fn refuses(&self) -> bool {
        self.available.is_some() && self.remote.is_some() && !self.allows
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
    use burrow_admin_proto::updates::{ClientBuild, RolloutStatus, RunMode};

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
                    name: "box".into(),
                    connected: true,
                    build: Some(ClientBuild {
                        run: RunMode::ByHand,
                        remote: RemoteUpdates::Off,
                        ..build("0.2.1")
                    }),
                    available: Some("0.2.2".into()),
                    blocked: Some("remote updates are off in its config".into()),
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
    fn a_table_row_reads_short_and_marks_updates_off_only_when_one_is_waiting() {
        let st = status();
        let nas = of(Some(&st), "nas").unwrap();
        assert_eq!(nas.system_short(), "linux/arm64 · service");
        assert!(!nas.refuses());
        let boxed = of(Some(&st), "box").unwrap();
        assert_eq!(boxed.system_short(), "linux/arm64 · by hand");
        assert_eq!(
            boxed.summary(),
            "linux/arm64 · by hand · remote updates off in its config"
        );
        assert!(boxed.refuses());
        let mut quiet = st.clone();
        quiet.clients[2].available = None;
        assert!(!of(Some(&quiet), "box").unwrap().refuses());
        assert_eq!(of(Some(&st), "old").unwrap().system_short(), "");
    }

    #[test]
    fn a_status_push_that_only_ages_an_update_leaves_the_client_view_unchanged() {
        let job = |secs_ago| UpdateStatus {
            id: 1,
            from: "0.2.1".into(),
            to: "0.2.2".into(),
            state: UpdateState::Done,
            secs_ago,
            ..UpdateStatus::default()
        };
        let mut a = status();
        a.clients[0].update = Some(job(3));
        let mut b = status();
        b.clients[0].update = Some(job(4));
        assert_eq!(of(Some(&a), "nas"), of(Some(&b), "nas"));
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

    fn seen() -> Status {
        Status {
            uptime_secs: 900,
            clients: vec![
                ClientStatus {
                    name: "home".into(),
                    connected: true,
                    from: Some("[2001:db8::1]:51000".into()),
                    ..ClientStatus::default()
                },
                ClientStatus {
                    name: "office".into(),
                    left_secs_ago: Some(120),
                    ..ClientStatus::default()
                },
                ClientStatus {
                    name: "self".into(),
                    connected: true,
                    local: true,
                    ..ClientStatus::default()
                },
            ],
            ..Status::default()
        }
    }

    #[test]
    fn this_server_is_always_up_and_a_client_is_up_while_connected() {
        let st = seen();
        let reserved = vec!["self".to_string()];
        assert_eq!(presence(Some(&st), &reserved, "self"), Presence::Here);
        assert_eq!(presence(None, &reserved, "self"), Presence::Here);
        assert_eq!(presence(Some(&st), &[], "self"), Presence::Here);
        assert_eq!(presence(Some(&st), &reserved, "home"), Presence::Online);
        assert_eq!(presence(Some(&st), &reserved, "office"), Presence::Offline);
        assert_eq!(presence(Some(&st), &reserved, "gone"), Presence::Offline);
        assert_eq!(presence(None, &reserved, "home"), Presence::Unknown);
        assert!(online(Some(&st), &reserved, "self") && !online(None, &[], "home"));
    }

    #[test]
    fn an_address_reads_as_its_ip_the_same_for_ipv4_and_ipv6() {
        assert_eq!(from_ip("203.0.113.7:51000"), "203.0.113.7");
        assert_eq!(from_ip("[2001:db8::1]:443"), "2001:db8::1");
        assert_eq!(from_ip("[2001:db8::1]"), "2001:db8::1");
        assert_eq!(from_ip("2001:db8::1"), "2001:db8::1");
        assert_eq!(from_ip("203.0.113.7"), "203.0.113.7");
        assert_eq!(from_ip("[::ffff:203.0.113.7]:9"), "203.0.113.7");
        assert_eq!(from_ip("unix socket"), "unix socket");
    }

    #[test]
    fn a_detail_says_where_from_or_since_when() {
        let st = seen();
        let reserved = vec!["self".to_string()];
        assert_eq!(detail(Some(&st), &reserved, "home"), "online · 2001:db8::1");
        assert_eq!(detail(Some(&st), &reserved, "office"), "offline");
        assert_eq!(away_text(&st, "office", None), "left 2m ago");
        assert_eq!(detail(Some(&st), &reserved, "self"), "This server");
        assert_eq!(detail(None, &reserved, "home"), "");
        let home = of(Some(&st), "home").unwrap();
        assert!(home.online && !home.local);
        assert_eq!(home.from.as_deref(), Some("2001:db8::1"));
    }
}
