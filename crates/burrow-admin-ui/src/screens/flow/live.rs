use burrow_admin_logic::live::Live as LiveStatus;
use std::collections::HashMap;

use burrow_admin_logic::presence::{Presence, presence_with};
use burrow_admin_logic::rates::{Rate, Rates};
use burrow_admin_proto::status::{ClientStatus, ServiceStatus};

pub(super) struct Live<'a> {
    status: Option<&'a LiveStatus>,
    reserved: Vec<String>,
    rates: HashMap<&'a str, Rate>,
}

impl<'a> Live<'a> {
    pub(super) fn new(status: Option<&'a LiveStatus>, rates: &'a Rates) -> Self {
        Live {
            status,
            reserved: crate::format::reserved_clients(),
            rates: rates
                .services
                .iter()
                .map(|(n, r)| (n.as_str(), *r))
                .collect(),
        }
    }

    pub(super) fn known(&self) -> bool {
        self.status.is_some()
    }

    pub(super) fn client(&self, name: &str) -> Option<&'a ClientStatus> {
        self.status.and_then(|s| s.client(name))
    }

    pub(super) fn service(&self, name: &str) -> Option<&'a ServiceStatus> {
        self.status.and_then(|s| s.service(name))
    }

    pub(super) fn rate_in(&self, name: &str) -> f64 {
        self.rates.get(name).map_or(0.0, |r| r.bytes_in)
    }

    pub(super) fn presence(&self, name: &str) -> Presence {
        presence_with(self.known(), &self.reserved, name, self.client(name))
    }

    pub(super) fn up(&self, name: &str) -> bool {
        self.presence(name).up()
    }

    pub(super) fn connected(&self, name: &str) -> bool {
        presence_with(self.known(), &[], name, self.client(name)).up()
    }
}
