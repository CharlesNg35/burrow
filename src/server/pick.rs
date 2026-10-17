use std::sync::Arc;

use super::{ClientSession, Shared};
use crate::config::model::Mode;
use crate::config::settings::Service;

impl Shared {
    pub(crate) fn pick(&self, service: &str, svc: &Service) -> Option<Arc<ClientSession>> {
        let pool = self.candidates(service, svc);
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

    pub(crate) fn preferred(&self, service: &str, svc: &Service) -> Option<String> {
        match svc.mode {
            Mode::Failover => self
                .candidates(service, svc)
                .first()
                .map(|s| s.name.clone()),
            Mode::Balance => None,
        }
    }

    fn candidates(&self, service: &str, svc: &Service) -> Vec<Arc<ClientSession>> {
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
        if up.is_empty() { connected } else { up }
    }

    pub(crate) fn still_open(
        &self,
        service: &str,
        client: &str,
        from: std::net::IpAddr,
        index: u16,
    ) -> Result<(), String> {
        let settings = self.settings();
        let Some(s) = settings.services.get(service) else {
            return Err("the service was removed".into());
        };
        if !s.clients.iter().any(|c| c == client) {
            return Err(format!("client \"{client}\" no longer serves it"));
        }
        if !s.sources.permits(from) {
            return Err(format!(
                "{from} is no longer allowed by allow_from or deny_from"
            ));
        }
        if index >= s.ports.max(1) {
            return Err("its port was removed from the range".into());
        }
        Ok(())
    }
}
