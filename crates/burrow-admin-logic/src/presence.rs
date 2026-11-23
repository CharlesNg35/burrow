//! Whether a client is here, online or offline, and how a client's version, system and updates read in the admin UI.

use crate::live::Live;
use std::net::{IpAddr, SocketAddr};

use burrow_admin_proto::status::{ClientStatus, OFFLINE, ONLINE, Status, away};
use burrow_admin_proto::updates::{
    FIRST_REPORTING, RemoteUpdates, Shown, UpdateState, UpdateStatus, hand_hint, job_text,
    platform_text, remote_text, run_text, state_text, version, version_text,
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
    pub failed_recently: bool,
}

pub fn client<'a>(status: &'a Live, name: &str) -> Option<&'a ClientStatus> {
    status.client(name)
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
        failed_recently: c
            .update
            .as_ref()
            .is_some_and(|u| crate::own_update::recent_failure(u.state, u.secs_ago)),
    }
}

pub fn of(status: Option<&Live>, name: &str) -> Option<ClientInfo> {
    status.and_then(|s| client(s, name)).map(info)
}

pub fn presence(status: Option<&Live>, reserved: &[String], name: &str) -> Presence {
    presence_with(
        status.is_some(),
        reserved,
        name,
        status.and_then(|s| client(s, name)),
    )
}

pub fn presence_with(
    known: bool,
    reserved: &[String],
    name: &str,
    found: Option<&ClientStatus>,
) -> Presence {
    if reserved.iter().any(|r| r == name) {
        return Presence::Here;
    }
    if !known {
        return Presence::Unknown;
    }
    match found {
        Some(c) if c.local => Presence::Here,
        Some(c) if c.connected => Presence::Online,
        _ => Presence::Offline,
    }
}

pub fn online(status: Option<&Live>, reserved: &[String], name: &str) -> bool {
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

pub fn detail(status: Option<&Live>, reserved: &[String], name: &str) -> String {
    detail_with(
        status.is_some(),
        reserved,
        name,
        status.and_then(|s| client(s, name)),
    )
}

pub fn detail_with(
    known: bool,
    reserved: &[String],
    name: &str,
    found: Option<&ClientStatus>,
) -> String {
    match (presence_with(known, reserved, name, found), found) {
        (Presence::Online, Some(c)) => c.from.as_deref().map_or_else(
            || ONLINE.to_string(),
            |f| format!("{ONLINE} · {}", from_ip(f)),
        ),
        (p, _) => p.word().to_string(),
    }
}

pub fn away_text(status: &Live, name: &str, added_secs_ago: Option<u64>) -> String {
    let left = client(status, name).and_then(|c| c.left_secs_ago);
    away(
        left,
        status.uptime_secs,
        added_secs_ago,
        burrow_syntax::duration::Elapsed::Coarse,
    )
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
        let mut line = job_text(u);
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Card {
    pub tag: Option<(String, Mood, String)>,
    pub can_ask: bool,
}

impl ClientInfo {
    pub fn card(&self, offered: bool) -> Card {
        let ready = offered && self.ready();
        let job = |u: &UpdateStatus| {
            let mut line = job_text(u);
            if !u.by.is_empty() {
                line.push_str(&format!(", asked by {}", u.by));
            }
            line
        };
        use crate::own_update::{Say, say};
        let says = say(
            self.update.as_ref().is_some_and(|u| !u.state.finished()),
            self.update
                .as_ref()
                .map(|u| (u.state, if self.failed_recently { 0 } else { u64::MAX })),
            self.available.is_some(),
        );
        let tag = |t: String, m: Mood| Card {
            tag: Some((t, m, self.update.as_ref().map(job).unwrap_or_default())),
            can_ask: false,
        };
        match (says, &self.update, &self.available) {
            (Say::Running, ..) => tag("updating…".into(), Mood::Busy),
            (Say::Failed, Some(u), _) => Card {
                tag: Some((
                    state_text(u.state).into(),
                    Mood::Offer,
                    crate::update_words::trouble(u, Some(&self.name))
                        .map(|t| format!("{} See Needs attention.", t.sentence))
                        .unwrap_or_default(),
                )),
                can_ask: false,
            },
            (Say::Available, _, Some(v)) => Card {
                tag: Some((
                    format!("{v} available"),
                    if self.blocked.is_some() {
                        Mood::Quiet
                    } else {
                        Mood::Offer
                    },
                    self.blocked.clone().unwrap_or_default(),
                )),
                can_ask: ready,
            },
            _ => Card::default(),
        }
    }
}

pub fn skipped(status: &Status) -> Vec<(String, String)> {
    status
        .clients
        .iter()
        .filter(|c| !c.local && c.connected)
        .map(info)
        .filter_map(|c| match (&c.available, &c.blocked) {
            (Some(_), Some(why)) => Some((c.name, why.clone())),
            (None, _) if c.older => Some((c.name, format!("older than {FIRST_REPORTING}"))),
            _ => None,
        })
        .collect()
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

pub fn rollout_line(st: &Live) -> Option<(String, Mood)> {
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
mod tests;
