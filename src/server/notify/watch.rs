use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use burrow_admin_proto::status::{CertStateStatus, Status};
use burrow_config::NotifyEvent;

use crate::metrics::render::duration;

const CLIENT_GRACE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Client,
    Service,
    Certificate,
    CountryDatabase,
}

impl Kind {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Kind::Client => "client",
            Kind::Service => "service",
            Kind::Certificate => "certificate",
            Kind::CountryDatabase => "geoip",
        }
    }

    fn events(self) -> (NotifyEvent, NotifyEvent) {
        match self {
            Kind::Client => (NotifyEvent::ClientOffline, NotifyEvent::ClientBack),
            Kind::Service => (NotifyEvent::SiteDown, NotifyEvent::SiteBack),
            Kind::Certificate => (
                NotifyEvent::CertificateFailing,
                NotifyEvent::CertificateFailing,
            ),
            Kind::CountryDatabase => (NotifyEvent::GeoipStale, NotifyEvent::GeoipStale),
        }
    }

    fn grace(self) -> Duration {
        match self {
            Kind::Client => CLIENT_GRACE,
            Kind::Service | Kind::Certificate | Kind::CountryDatabase => Duration::ZERO,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Health {
    Problem(String),
    Fine(String),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) event: NotifyEvent,
    pub(crate) resolved: bool,
    pub(crate) kind: Kind,
    pub(crate) name: String,
    pub(crate) title: String,
    pub(crate) text: String,
}

struct Track {
    since: Instant,
    told: bool,
}

#[derive(Default)]
pub(crate) struct Watch {
    open: BTreeMap<(Kind, String), Track>,
}

impl Watch {
    pub(crate) fn step(
        &mut self,
        now: Instant,
        seen: BTreeMap<(Kind, String), Health>,
    ) -> Vec<Message> {
        let mut out = Vec::new();
        self.open.retain(|key, _| seen.contains_key(key));
        for (key, health) in seen {
            match health {
                Health::Problem(detail) => {
                    let t = self.open.entry(key.clone()).or_insert(Track {
                        since: now,
                        told: false,
                    });
                    if !t.told && now.duration_since(t.since) >= key.0.grace() {
                        t.told = true;
                        out.push(problem(&key, &detail));
                    }
                }
                Health::Fine(detail) => {
                    if let Some(t) = self.open.remove(&key)
                        && t.told
                    {
                        out.push(resolved(&key, &detail, now.duration_since(t.since)));
                    }
                }
                Health::Unknown => {}
            }
        }
        out
    }
}

fn problem((kind, name): &(Kind, String), detail: &str) -> Message {
    let (title, text) = match kind {
        Kind::Client => (
            format!("Client {name} is offline"),
            format!(
                "Client \"{name}\" lost its connection and has not come back for {}.",
                duration(CLIENT_GRACE.as_secs())
            ),
        ),
        Kind::Service => (
            format!("Service {name} is down"),
            format!("Service \"{name}\" fails its health check on every client: {detail}."),
        ),
        Kind::Certificate => (
            format!("The certificate for {name} is failing"),
            format!("The certificate for {name} cannot be got or renewed: {detail}."),
        ),
        Kind::CountryDatabase => (
            "The country database is out of date".to_string(),
            format!(
                "The country database ({name}) is not being kept up to date: {detail}. Country rules use the last good copy, or refuse public visitors under allow_countries when there is none."
            ),
        ),
    };
    Message {
        event: kind.events().0,
        resolved: false,
        kind: *kind,
        name: name.clone(),
        title,
        text,
    }
}

fn resolved((kind, name): &(Kind, String), detail: &str, after: Duration) -> Message {
    let after = duration(after.as_secs());
    let (title, text) = match kind {
        Kind::Client => (
            format!("Client {name} is back"),
            format!("Client \"{name}\" is connected again{detail}, after {after} offline."),
        ),
        Kind::Service => (
            format!("Service {name} is back"),
            format!("Service \"{name}\" answers its health check again, after {after} down."),
        ),
        Kind::Certificate => (
            format!("The certificate for {name} works again"),
            format!("The certificate for {name} is fine again, after {after} of failures."),
        ),
        Kind::CountryDatabase => (
            "The country database is up to date again".to_string(),
            format!("The country database ({name}) is fine again: {detail}, after {after}."),
        ),
    };
    Message {
        event: kind.events().1,
        resolved: true,
        kind: *kind,
        name: name.clone(),
        title,
        text,
    }
}

pub(crate) fn seen(status: &Status) -> BTreeMap<(Kind, String), Health> {
    let mut out = BTreeMap::new();
    for c in status.clients.iter().filter(|c| !c.local) {
        let health = match (c.connected, c.left_secs_ago) {
            (true, _) => Health::Fine(
                c.from
                    .as_ref()
                    .map(|f| format!(" from {f}"))
                    .unwrap_or_default(),
            ),
            (false, Some(_)) => Health::Problem(String::new()),
            (false, None) => Health::Unknown,
        };
        out.insert((Kind::Client, c.name.clone()), health);
    }
    for s in &status.services {
        let health = match s.health.iter().find(|h| h.up) {
            Some(_) => Health::Fine(String::new()),
            None if s.health.is_empty() => Health::Unknown,
            None => Health::Problem(
                s.health
                    .iter()
                    .find_map(|h| h.reason.clone())
                    .unwrap_or_else(|| "no answer".into()),
            ),
        };
        out.insert((Kind::Service, s.name.clone()), health);
    }
    for c in &status.certificates {
        let health = match &c.state {
            CertStateStatus::Failing { reason } => Health::Problem(reason.clone()),
            CertStateStatus::Ok => Health::Fine(String::new()),
            _ => Health::Unknown,
        };
        out.insert((Kind::Certificate, c.name.clone()), health);
    }
    if let Some(g) = &status.geoip {
        let line = g.line(burrow_syntax::duration::now_secs());
        let health = if g.stale || (g.failed && !g.ready()) {
            Health::Problem(line)
        } else if g.ready() {
            Health::Fine(line)
        } else {
            Health::Unknown
        };
        out.insert((Kind::CountryDatabase, g.source.clone()), health);
    }
    out
}

#[cfg(test)]
mod tests;
