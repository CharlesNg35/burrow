use std::sync::Arc;

use super::{ClientSession, Shared};
use crate::config::model::Mode;
use crate::config::settings::Service;

impl Shared {
    pub(crate) fn pick(&self, service: &str, svc: &Service) -> Option<Arc<ClientSession>> {
        let connected: Vec<Arc<ClientSession>> = {
            let sessions = self.sessions.lock().expect("sessions lock");
            svc.clients
                .iter()
                .filter_map(|c| sessions.get(c).cloned())
                .collect()
        };
        let up: Vec<Arc<ClientSession>> = connected
            .iter()
            .filter(|s| svc.health.is_none() || self.is_up(service, &s.name))
            .cloned()
            .collect();
        let pool = if up.is_empty() { connected } else { up };
        if pool.is_empty() {
            return None;
        }
        let at = match svc.mode {
            Mode::Failover => 0,
            Mode::Balance => {
                let mut turns = self.turns.lock().expect("turns lock");
                let turn = turns.entry(service.to_string()).or_default();
                *turn = turn.wrapping_add(1);
                *turn % pool.len()
            }
        };
        Some(pool[at].clone())
    }

    pub(crate) fn still_serves(&self, service: &str, client: &str) -> bool {
        self.settings()
            .services
            .get(service)
            .is_some_and(|s| s.clients.iter().any(|c| c == client))
    }
}
