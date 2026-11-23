use std::collections::HashMap;

use burrow_admin_logic::presence::{Presence, presence_with};
use burrow_admin_proto::status::{ClientStatus, ServiceStatus, Status};

pub(super) struct Live<'a> {
    pub(super) status: Option<&'a Status>,
    clients: HashMap<&'a str, &'a ClientStatus>,
    services: HashMap<&'a str, &'a ServiceStatus>,
    reserved: Vec<String>,
}

impl<'a> Live<'a> {
    pub(super) fn new(status: Option<&'a Status>) -> Self {
        Live {
            status,
            clients: status
                .map(|s| s.clients.iter().map(|c| (c.name.as_str(), c)).collect())
                .unwrap_or_default(),
            services: status
                .map(|s| s.services.iter().map(|x| (x.name.as_str(), x)).collect())
                .unwrap_or_default(),
            reserved: crate::format::reserved_clients(),
        }
    }

    pub(super) fn known(&self) -> bool {
        self.status.is_some()
    }

    pub(super) fn client(&self, name: &str) -> Option<&'a ClientStatus> {
        self.clients.get(name).copied()
    }

    pub(super) fn service(&self, name: &str) -> Option<&'a ServiceStatus> {
        self.services.get(name).copied()
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
