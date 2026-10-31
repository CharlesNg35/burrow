use std::ops::Deref;
use std::sync::Arc;

use super::standing::Seen;
use super::via::Via;
use super::{CountGuard, Shared};
use crate::config::settings::{EntryKey, Service};
use burrow_config::Mode;

#[derive(Clone)]
pub(crate) struct Hop {
    pub(crate) via: Via,
    pub(crate) target: String,
    pub(crate) key: EntryKey,
    pub(crate) weight: u32,
}

impl Deref for Hop {
    type Target = Via;

    fn deref(&self) -> &Via {
        &self.via
    }
}

impl Hop {
    pub(crate) fn direct(via: Via, target: &str) -> Hop {
        Hop {
            key: EntryKey {
                client: Arc::from(via.name()),
                base: Arc::from(target),
            },
            via,
            target: target.to_string(),
            weight: 1,
        }
    }
}

pub(crate) struct Picked {
    pub(crate) hops: Vec<Hop>,
    held: Option<Held>,
}

pub(super) struct Held {
    pub(super) name: String,
    pub(super) target: String,
    pub(super) guard: CountGuard,
    pub(super) entry: Option<CountGuard>,
    pub(super) before: usize,
}

impl Picked {
    pub(crate) fn is_empty(&self) -> bool {
        self.hops.is_empty()
    }

    pub(super) fn split(self) -> (Vec<Hop>, Option<Held>) {
        (self.hops, self.held)
    }
}

impl From<Vec<Hop>> for Picked {
    fn from(hops: Vec<Hop>) -> Self {
        Picked { hops, held: None }
    }
}

impl Shared {
    pub(crate) fn route(&self, service: &str, svc: &Service, index: u16) -> Picked {
        let (hops, held) = self.order(service, svc, index, true);
        Picked { hops, held }
    }

    pub(crate) fn pick(&self, service: &str, svc: &Service, index: u16) -> Option<Hop> {
        self.route(service, svc, index).hops.into_iter().next()
    }

    pub(crate) fn preferred(
        &self,
        service: &str,
        svc: &Service,
        index: u16,
    ) -> Option<(String, String)> {
        match svc.mode {
            Mode::Failover => self
                .order(service, svc, index, false)
                .0
                .first()
                .map(|h| (h.name().to_string(), h.target.clone())),
            Mode::Balance | Mode::LeastConnections => None,
        }
    }

    pub(crate) fn has_clear(&self, service: &str, svc: &Service, index: u16) -> bool {
        self.hops(svc, index).iter().any(|h| {
            (svc.health.is_none() || self.is_up(service, &h.key))
                && self.seen(service, h, &h.target) == Seen::Clear
        })
    }

    fn hops(&self, svc: &Service, index: u16) -> Vec<Hop> {
        let sessions = self.connected(&svc.clients);
        svc.entries()
            .iter()
            .filter_map(|e| {
                let via = sessions.iter().find(|v| v.name() == e.client)?;
                Some(Hop {
                    via: via.clone(),
                    target: svc.entry_target_at(e, index),
                    key: e.key.clone(),
                    weight: e.weight,
                })
            })
            .collect()
    }

    fn order(
        &self,
        service: &str,
        svc: &Service,
        index: u16,
        hold: bool,
    ) -> (Vec<Hop>, Option<Held>) {
        let (mut clear, mut aside, mut refused) = (Vec::new(), Vec::new(), Vec::new());
        for h in self.hops(svc, index) {
            let healthy = svc.health.is_none() || self.is_up(service, &h.key);
            match self.seen(service, &h, &h.target) {
                Seen::Refused => refused.push(h),
                Seen::Aside => aside.push(h),
                Seen::Clear if healthy => clear.push(h),
                Seen::Clear => aside.push(h),
            }
        }
        let mut wheels =
            (hold && svc.mode != Mode::Failover).then(|| self.wheels.lock().expect("wheels lock"));
        let head = [&mut clear, &mut aside, &mut refused]
            .into_iter()
            .find(|g| !g.is_empty());
        if let Some(head) = head
            && hold
        {
            match svc.mode {
                Mode::Failover => {}
                Mode::Balance => {
                    if let Some(wheels) = wheels.as_mut() {
                        let group = head.iter().map(|h| (&h.key, h.weight));
                        let first = wheels.turn(service, svc.entries_id(), group);
                        head.rotate_left(first);
                    }
                }
                Mode::LeastConnections => {
                    head.sort_by_cached_key(|h| self.entry_open(service, &h.key));
                }
            }
        }
        clear.extend(aside);
        clear.extend(refused);
        let held = clear.first().filter(|_| hold).map(|h| {
            let (guard, before) = self.client_opens(h.name());
            Held {
                name: h.name().to_string(),
                target: h.target.clone(),
                guard,
                entry: Some(self.entry_opens(service, h)),
                before,
            }
        });
        drop(wheels);
        (clear, held)
    }

    pub(super) fn entry_opens(&self, service: &str, hop: &Hop) -> CountGuard {
        self.entry_counts.enter(service, &hop.key)
    }

    pub(super) fn entry_open(&self, service: &str, key: &EntryKey) -> u64 {
        self.entry_counts.current(service, key)
    }

    #[cfg(feature = "web")]
    pub(crate) fn still_on_web(&self, service: &str, host: &str) -> Result<(), String> {
        let settings = self.settings();
        let site = settings.web.as_ref().and_then(|w| w.site_of(service));
        match site {
            Some(s) if s.hosts.iter().any(|h| burrow_syntax::web::covers(h, host)) => Ok(()),
            _ => Err(format!("\"{host}\" no longer leads to it on the web proxy")),
        }
    }

    pub(crate) fn still_open(
        &self,
        service: &str,
        client: &str,
        from: std::net::IpAddr,
        index: u16,
        public: Option<(burrow_config::Protocol, std::net::SocketAddr)>,
    ) -> Result<(), String> {
        let settings = self.settings();
        let Some(s) = settings.services.get(service) else {
            return Err("the service was removed".into());
        };
        if let Some((protocol, bind)) = public
            && (s.protocol != protocol
                || s.bind.is_none()
                || (bind.port() != 0 && s.bind != Some(bind)))
        {
            return Err(format!("the service no longer listens on {bind}"));
        }
        if !s.clients.iter().any(|c| c == client) {
            return Err(format!("client \"{client}\" no longer serves it"));
        }
        if !self.admits(s, from).admitted() {
            return Err(format!(
                "{from} is no longer allowed by allow_from, deny_from or the country rules"
            ));
        }
        if index >= s.ports.max(1) {
            return Err("its port was removed from the range".into());
        }
        Ok(())
    }
}
