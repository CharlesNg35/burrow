use super::standing::Seen;
use super::via::Via;
use super::{CountGuard, Shared};
use crate::config::settings::Service;
use burrow_config::Mode;

pub(crate) struct Picked {
    pub(crate) vias: Vec<Via>,
    held: Option<Held>,
}

pub(super) struct Held {
    pub(super) name: String,
    pub(super) guard: CountGuard,
    pub(super) before: usize,
}

impl Picked {
    pub(crate) fn is_empty(&self) -> bool {
        self.vias.is_empty()
    }

    pub(super) fn split(self) -> (Vec<Via>, Option<Held>) {
        (self.vias, self.held)
    }
}

impl From<Vec<Via>> for Picked {
    fn from(vias: Vec<Via>) -> Self {
        Picked { vias, held: None }
    }
}

impl Shared {
    pub(crate) fn route(&self, service: &str, svc: &Service, target: &str) -> Picked {
        let (vias, held) = self.order(service, svc, target, true);
        Picked { vias, held }
    }

    pub(crate) fn pick(&self, service: &str, svc: &Service, target: &str) -> Option<Via> {
        self.route(service, svc, target).vias.into_iter().next()
    }

    pub(crate) fn preferred(&self, service: &str, svc: &Service, target: &str) -> Option<String> {
        match svc.mode {
            Mode::Failover => self
                .order(service, svc, target, false)
                .0
                .first()
                .map(|s| s.name().to_string()),
            Mode::Balance | Mode::LeastConnections => None,
        }
    }

    pub(crate) fn has_clear(&self, service: &str, svc: &Service, target: &str) -> bool {
        self.connected(&svc.clients).iter().any(|s| {
            (svc.health.is_none() || self.is_up(service, s.name()))
                && self.seen(service, s, target) == Seen::Clear
        })
    }

    fn order(
        &self,
        service: &str,
        svc: &Service,
        target: &str,
        hold: bool,
    ) -> (Vec<Via>, Option<Held>) {
        let connected = self.connected(&svc.clients);
        let (mut clear, mut aside, mut refused) = (Vec::new(), Vec::new(), Vec::new());
        for s in connected {
            let healthy = svc.health.is_none() || self.is_up(service, s.name());
            match self.seen(service, &s, target) {
                Seen::Refused => refused.push(s),
                Seen::Aside => aside.push(s),
                Seen::Clear if healthy => clear.push(s),
                Seen::Clear => aside.push(s),
            }
        }
        let mut wheels = self.wheels.lock().expect("wheels lock");
        let head = [&mut clear, &mut aside, &mut refused]
            .into_iter()
            .find(|g| !g.is_empty());
        if let Some(head) = head
            && hold
        {
            match svc.mode {
                Mode::Failover => {}
                Mode::Balance => {
                    let first = {
                        let group: Vec<(&str, u32)> = head
                            .iter()
                            .map(|v| (v.name(), svc.weight_of(v.name())))
                            .collect();
                        wheels.turn(service, svc, &group)
                    };
                    head.rotate_left(first);
                }
                Mode::LeastConnections => {
                    head.sort_by_cached_key(|v| self.client_counts.current(v.name()));
                }
            }
        }
        clear.extend(aside);
        clear.extend(refused);
        let held = clear.first().filter(|_| hold).map(|v| {
            let (guard, before) = self.client_opens(v.name());
            Held {
                name: v.name().to_string(),
                guard,
                before,
            }
        });
        drop(wheels);
        (clear, held)
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
