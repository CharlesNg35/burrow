use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::status::{CertStatus, ClientStatus, EntryStatus, ServiceStatus, VisitorStatus};

pub trait Row: Debug + Clone + PartialEq {
    type Counts: Debug + Clone + PartialEq + Serialize + DeserializeOwned;

    fn key(&self) -> &str;
    fn same_shape(&self, other: &Self) -> bool;
    fn counts(&self) -> Self::Counts;
    fn put_counts(&mut self, c: Self::Counts);

    fn age_counts(_c: &mut Self::Counts, _secs: u64) {}

    fn age(&mut self, _secs: u64) {}
}

fn older(v: &mut Option<u64>, secs: u64) {
    if let Some(v) = v.as_mut() {
        *v = v.saturating_add(secs);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceCounts(
    pub u64,
    pub u64,
    pub u64,
    pub u64,
    pub u64,
    pub Option<[u64; 5]>,
    pub Vec<u64>,
);

impl Row for ServiceStatus {
    type Counts = ServiceCounts;

    fn key(&self) -> &str {
        &self.name
    }

    fn same_shape(&self, o: &Self) -> bool {
        let ServiceStatus {
            name,
            protocol,
            bind,
            target,
            clients,
            mode,
            bandwidth,
            visitors,
            proxy_protocol,
            filtered,
            open: _,
            total: _,
            refused: _,
            bytes_in: _,
            bytes_out: _,
            health,
            set_aside,
            entries,
            web,
            http,
            file,
            not_listening,
        } = self;
        *name == o.name
            && *protocol == o.protocol
            && *bind == o.bind
            && *target == o.target
            && *clients == o.clients
            && *mode == o.mode
            && *bandwidth == o.bandwidth
            && *visitors == o.visitors
            && *proxy_protocol == o.proxy_protocol
            && *filtered == o.filtered
            && *health == o.health
            && *set_aside == o.set_aside
            && same_entries(entries, &o.entries)
            && *web == o.web
            && http.is_some() == o.http.is_some()
            && *file == o.file
            && *not_listening == o.not_listening
    }

    fn counts(&self) -> ServiceCounts {
        ServiceCounts(
            self.open,
            self.total,
            self.refused,
            self.bytes_in,
            self.bytes_out,
            self.http.as_ref().map(|h| h.responses),
            self.entries.iter().map(|e| e.open).collect(),
        )
    }

    fn put_counts(&mut self, c: ServiceCounts) {
        let ServiceCounts(open, total, refused, bytes_in, bytes_out, http, entries) = c;
        self.open = open;
        self.total = total;
        self.refused = refused;
        self.bytes_in = bytes_in;
        self.bytes_out = bytes_out;
        if let (Some(h), Some(r)) = (self.http.as_mut(), http) {
            h.responses = r;
        }
        for (e, open) in self.entries.iter_mut().zip(entries) {
            e.open = open;
        }
    }
}

fn same_entries(a: &[EntryStatus], b: &[EntryStatus]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            let EntryStatus {
                client,
                target,
                open: _,
            } = x;
            *client == y.client && *target == y.target
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCounts(pub Option<u64>, pub Option<u64>, pub u64, pub u64, pub u64);

impl Row for ClientStatus {
    type Counts = ClientCounts;

    fn key(&self) -> &str {
        &self.name
    }

    fn same_shape(&self, o: &Self) -> bool {
        let ClientStatus {
            name,
            connected,
            connected_secs: _,
            from,
            open: _,
            idle_channels: _,
            left_secs_ago: _,
            local,
            busy_total: _,
            at_limit,
            allow,
            build,
            update,
            available,
            blocked,
        } = self;
        *name == o.name
            && *connected == o.connected
            && *from == o.from
            && *local == o.local
            && *at_limit == o.at_limit
            && *allow == o.allow
            && *build == o.build
            && *update == o.update
            && *available == o.available
            && *blocked == o.blocked
    }

    fn counts(&self) -> ClientCounts {
        ClientCounts(
            self.connected_secs,
            self.left_secs_ago,
            self.open,
            self.idle_channels,
            self.busy_total,
        )
    }

    fn put_counts(&mut self, c: ClientCounts) {
        let ClientCounts(connected_secs, left_secs_ago, open, idle_channels, busy_total) = c;
        self.connected_secs = connected_secs;
        self.left_secs_ago = left_secs_ago;
        self.open = open;
        self.idle_channels = idle_channels;
        self.busy_total = busy_total;
    }

    fn age_counts(c: &mut ClientCounts, secs: u64) {
        older(&mut c.0, secs);
        older(&mut c.1, secs);
    }

    fn age(&mut self, secs: u64) {
        older(&mut self.connected_secs, secs);
        older(&mut self.left_secs_ago, secs);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitorCounts(pub u64, pub u64, pub Option<u64>);

impl Row for VisitorStatus {
    type Counts = VisitorCounts;

    fn key(&self) -> &str {
        &self.name
    }

    fn same_shape(&self, o: &Self) -> bool {
        self.name == o.name
    }

    fn counts(&self) -> VisitorCounts {
        VisitorCounts(self.open, self.total, self.last_seen_secs)
    }

    fn put_counts(&mut self, c: VisitorCounts) {
        let VisitorCounts(open, total, last_seen_secs) = c;
        self.open = open;
        self.total = total;
        self.last_seen_secs = last_seen_secs;
    }

    fn age_counts(c: &mut VisitorCounts, secs: u64) {
        older(&mut c.2, secs);
    }

    fn age(&mut self, secs: u64) {
        older(&mut self.last_seen_secs, secs);
    }
}

impl Row for CertStatus {
    type Counts = ();

    fn key(&self) -> &str {
        &self.name
    }

    fn same_shape(&self, o: &Self) -> bool {
        self == o
    }

    fn counts(&self) {}

    fn put_counts(&mut self, _c: ()) {}
}
