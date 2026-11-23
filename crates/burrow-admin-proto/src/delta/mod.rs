//! Live status as deltas: one full snapshot, then per tick only what changed, keyed by name.
//! The server keeps the last status it sent per session (`Leader`); the admin follows the sequence and asks
//! for a new snapshot on a gap (`Follower`). `diff` and `apply` are pure and round-trip exactly.

mod rows;

use std::collections::{HashMap, HashSet};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub use rows::{ClientCounts, Row, ServiceCounts, VisitorCounts};

use crate::status::{
    AcmeStatus, CertStatus, ClientStatus, LocalCaStatus, PortWait, ServiceStatus, Status,
    VisitorStatus,
};
use crate::updates::{LatestRelease, LocalClient, RolloutStatus, ServerUpdate};
use crate::web::WebSummary;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusUpdate {
    Full(Snapshot),
    Delta(Box<StatusDelta>),
}

impl StatusUpdate {
    pub fn seq(&self) -> u64 {
        match self {
            StatusUpdate::Full(s) => s.seq,
            StatusUpdate::Delta(d) => d.seq,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub seq: u64,
    pub status: Box<Status>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StatusDelta {
    pub seq: u64,
    pub up: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Box<Head>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<Box<WebPart>>,
    #[serde(default, skip_serializing_if = "Rows::is_empty")]
    pub services: Rows<ServiceStatus>,
    #[serde(default, skip_serializing_if = "Rows::is_empty")]
    pub clients: Rows<ClientStatus>,
    #[serde(default, skip_serializing_if = "Rows::is_empty")]
    pub visitors: Rows<VisitorStatus>,
    #[serde(default, skip_serializing_if = "Rows::is_empty")]
    pub certificates: Rows<CertStatus>,
}

impl StatusDelta {
    pub fn is_empty(&self) -> bool {
        self.head.is_none()
            && self.web.is_none()
            && self.services.is_empty()
            && self.clients.is_empty()
            && self.visitors.is_empty()
            && self.certificates.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Head {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub listening: Vec<String>,
    #[serde(default)]
    pub handshakes_refused: u64,
    #[serde(default)]
    pub udp_peers: u64,
    #[serde(default)]
    pub acme: Option<AcmeStatus>,
    #[serde(default)]
    pub local_ca: Option<LocalCaStatus>,
    #[serde(default)]
    pub web_waiting: Option<PortWait>,
    #[serde(default)]
    pub client: Option<LocalClient>,
    #[serde(default)]
    pub rollout: Option<RolloutStatus>,
    #[serde(default)]
    pub latest: Option<LatestRelease>,
    #[serde(default)]
    pub server_update: Option<ServerUpdate>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WebPart {
    #[serde(default)]
    pub summary: Option<WebSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(
    serialize = "T: Serialize, T::Counts: Serialize",
    deserialize = "T: DeserializeOwned, T::Counts: DeserializeOwned"
))]
pub struct Rows<T: Row> {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gone: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<(usize, T)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<T>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counts: Vec<(String, T::Counts)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<T>>,
}

impl<T: Row> Default for Rows<T> {
    fn default() -> Self {
        Rows {
            gone: Vec::new(),
            add: Vec::new(),
            set: Vec::new(),
            counts: Vec::new(),
            all: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mismatch;

impl<T: Row> Rows<T> {
    pub fn is_empty(&self) -> bool {
        self.gone.is_empty()
            && self.add.is_empty()
            && self.set.is_empty()
            && self.counts.is_empty()
            && self.all.is_none()
    }

    fn whole(new: &[T]) -> Self {
        Rows {
            all: Some(new.to_vec()),
            ..Rows::default()
        }
    }

    pub fn diff(old: &[T], new: &[T], secs: u64) -> Self {
        let old_at: HashMap<&str, usize> =
            old.iter().enumerate().map(|(i, r)| (r.key(), i)).collect();
        let new_keys: HashSet<&str> = new.iter().map(Row::key).collect();
        if old_at.len() != old.len() || new_keys.len() != new.len() {
            return Rows::whole(new);
        }
        let mut out = Rows::default();
        let mut last = None;
        for (i, n) in new.iter().enumerate() {
            let Some(&j) = old_at.get(n.key()) else {
                out.add.push((i, n.clone()));
                continue;
            };
            if last.is_some_and(|l| j < l) {
                return Rows::whole(new);
            }
            last = Some(j);
            let o = &old[j];
            if !o.same_shape(n) {
                out.set.push(n.clone());
                continue;
            }
            let mut was = o.counts();
            T::age_counts(&mut was, secs);
            let now = n.counts();
            if was != now {
                out.counts.push((n.key().to_string(), now));
            }
        }
        out.gone = old
            .iter()
            .filter(|o| !new_keys.contains(o.key()))
            .map(|o| o.key().to_string())
            .collect();
        out
    }

    pub fn apply(self, list: &mut Vec<T>, secs: u64) -> Result<(), Mismatch> {
        if let Some(all) = self.all {
            *list = all;
            return Ok(());
        }
        if secs > 0 {
            list.iter_mut().for_each(|r| r.age(secs));
        }
        if !self.gone.is_empty() {
            let gone: HashSet<&str> = self.gone.iter().map(String::as_str).collect();
            let before = list.len();
            list.retain(|r| !gone.contains(r.key()));
            if before - list.len() != gone.len() {
                return Err(Mismatch);
            }
        }
        if !self.add.is_empty() {
            merge(list, self.add)?;
        }
        if self.set.is_empty() && self.counts.is_empty() {
            return Ok(());
        }
        let (sets, counts) = {
            let at: HashMap<&str, usize> =
                list.iter().enumerate().map(|(i, r)| (r.key(), i)).collect();
            let find = |k: &str| at.get(k).copied().ok_or(Mismatch);
            let sets = self
                .set
                .iter()
                .map(|r| find(r.key()))
                .collect::<Result<Vec<_>, _>>()?;
            let counts = self
                .counts
                .iter()
                .map(|(k, _)| find(k))
                .collect::<Result<Vec<_>, _>>()?;
            (sets, counts)
        };
        for (i, r) in sets.into_iter().zip(self.set) {
            list[i] = r;
        }
        for (i, (_, c)) in counts.into_iter().zip(self.counts) {
            list[i].put_counts(c);
        }
        Ok(())
    }
}

fn merge<T>(list: &mut Vec<T>, add: Vec<(usize, T)>) -> Result<(), Mismatch> {
    let total = list.len() + add.len();
    let mut out = Vec::with_capacity(total);
    let mut kept = std::mem::take(list).into_iter();
    let mut adds = add.into_iter().peekable();
    for i in 0..total {
        match adds.next_if(|(at, _)| *at == i) {
            Some((_, r)) => out.push(r),
            None => out.push(kept.next().ok_or(Mismatch)?),
        }
    }
    if adds.next().is_some() {
        return Err(Mismatch);
    }
    *list = out;
    Ok(())
}

fn head(s: &Status) -> Head {
    let Status {
        version,
        uptime_secs: _,
        listening,
        handshakes_refused,
        udp_peers,
        clients: _,
        services: _,
        visitors: _,
        web: _,
        certificates: _,
        acme,
        local_ca,
        web_waiting,
        client,
        rollout,
        latest,
        server_update,
    } = s;
    Head {
        version: version.clone(),
        listening: listening.clone(),
        handshakes_refused: *handshakes_refused,
        udp_peers: *udp_peers,
        acme: acme.clone(),
        local_ca: local_ca.clone(),
        web_waiting: web_waiting.clone(),
        client: client.clone(),
        rollout: rollout.clone(),
        latest: latest.clone(),
        server_update: server_update.clone(),
    }
}

fn put_head(s: &mut Status, h: Head) {
    let Head {
        version,
        listening,
        handshakes_refused,
        udp_peers,
        acme,
        local_ca,
        web_waiting,
        client,
        rollout,
        latest,
        server_update,
    } = h;
    s.version = version;
    s.listening = listening;
    s.handshakes_refused = handshakes_refused;
    s.udp_peers = udp_peers;
    s.acme = acme;
    s.local_ca = local_ca;
    s.web_waiting = web_waiting;
    s.client = client;
    s.rollout = rollout;
    s.latest = latest;
    s.server_update = server_update;
}

pub fn diff(old: &Status, new: &Status) -> StatusDelta {
    let secs = new.uptime_secs.saturating_sub(old.uptime_secs);
    let now = head(new);
    StatusDelta {
        seq: 0,
        up: new.uptime_secs,
        head: (head(old) != now).then(|| Box::new(now)),
        web: (old.web != new.web).then(|| {
            Box::new(WebPart {
                summary: new.web.clone(),
            })
        }),
        services: Rows::diff(&old.services, &new.services, secs),
        clients: Rows::diff(&old.clients, &new.clients, secs),
        visitors: Rows::diff(&old.visitors, &new.visitors, secs),
        certificates: Rows::diff(&old.certificates, &new.certificates, secs),
    }
}

pub fn apply(s: &mut Status, d: StatusDelta) -> Result<(), Mismatch> {
    let secs = d.up.saturating_sub(s.uptime_secs);
    s.uptime_secs = d.up;
    if let Some(h) = d.head {
        put_head(s, *h);
    }
    if let Some(w) = d.web {
        s.web = w.summary;
    }
    d.services.apply(&mut s.services, secs)?;
    d.clients.apply(&mut s.clients, secs)?;
    d.visitors.apply(&mut s.visitors, secs)?;
    d.certificates.apply(&mut s.certificates, secs)
}

#[derive(Debug, Default)]
pub struct Leader {
    seq: u64,
    last: Option<Status>,
}

impl Leader {
    pub fn update(&self, status: &Status) -> StatusUpdate {
        let seq = self.seq + 1;
        match &self.last {
            None => StatusUpdate::Full(Snapshot {
                seq,
                status: Box::new(status.clone()),
            }),
            Some(old) => StatusUpdate::Delta(Box::new(StatusDelta {
                seq,
                ..diff(old, status)
            })),
        }
    }

    pub fn sent(&mut self, status: Status) {
        self.seq += 1;
        self.last = Some(status);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Follow {
    Show(Box<Status>),
    Resync,
    Wait,
}

#[derive(Debug, Default)]
pub struct Follower {
    seq: Option<u64>,
    asked: bool,
}

impl Follower {
    pub fn take(&mut self, current: Option<&Status>, update: StatusUpdate) -> Follow {
        let d = match update {
            StatusUpdate::Full(s) => {
                self.seq = Some(s.seq);
                self.asked = false;
                return Follow::Show(s.status);
            }
            StatusUpdate::Delta(d) => *d,
        };
        if let (Some(seq), Some(cur)) = (self.seq, current)
            && d.seq == seq + 1
        {
            let mut next = cur.clone();
            let seq = d.seq;
            if apply(&mut next, d).is_ok() {
                self.seq = Some(seq);
                return Follow::Show(Box::new(next));
            }
        }
        self.seq = None;
        if std::mem::replace(&mut self.asked, true) {
            Follow::Wait
        } else {
            Follow::Resync
        }
    }
}

#[cfg(test)]
mod tests;
