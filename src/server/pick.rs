use super::Shared;
use super::standing::Seen;
use super::via::Via;
use crate::config::model::Mode;
use crate::config::settings::Service;

impl Shared {
    pub(crate) fn route(&self, service: &str, svc: &Service, target: &str) -> Vec<Via> {
        self.order(service, svc, target, true)
    }

    pub(crate) fn pick(&self, service: &str, svc: &Service, target: &str) -> Option<Via> {
        self.route(service, svc, target).into_iter().next()
    }

    pub(crate) fn preferred(&self, service: &str, svc: &Service, target: &str) -> Option<String> {
        match svc.mode {
            Mode::Failover => self
                .order(service, svc, target, false)
                .first()
                .map(|s| s.name().to_string()),
            Mode::Balance => None,
        }
    }

    pub(crate) fn has_clear(&self, service: &str, svc: &Service, target: &str) -> bool {
        self.connected(&svc.clients).iter().any(|s| {
            (svc.health.is_none() || self.is_up(service, s.name()))
                && self.seen(service, s, target) == Seen::Clear
        })
    }

    fn order(&self, service: &str, svc: &Service, target: &str, rotate: bool) -> Vec<Via> {
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
        let head = [&mut clear, &mut aside, &mut refused]
            .into_iter()
            .find(|g| !g.is_empty());
        if let Some(head) = head
            && rotate
            && svc.mode == Mode::Balance
        {
            let mut turns = self.turns.lock().expect("turns lock");
            let turn = turns.entry(service.to_string()).or_default();
            *turn = turn.wrapping_add(1);
            let len = head.len();
            head.rotate_left(*turn % len);
        }
        clear.extend(aside);
        clear.extend(refused);
        clear
    }

    pub(crate) fn still_open(
        &self,
        service: &str,
        client: &str,
        from: std::net::IpAddr,
        index: u16,
        public: Option<(crate::config::model::Protocol, std::net::SocketAddr)>,
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
