use std::collections::HashMap;
use std::sync::Arc;

use burrow_web::http::health::{Ask, HealthCheck};

use super::options::site_options;
use crate::config::settings::{ServerSettings, WebTls};
use crate::forward::{Incoming, pump_tcp};
use crate::wire::transport::Channel;

pub(in crate::server) type Http = Arc<Result<HealthCheck, String>>;

const AGENT: &str = concat!("burrow-health-check/", env!("CARGO_PKG_VERSION"));

#[derive(Default)]
pub(in crate::server) struct Checks {
    of: Option<Arc<ServerSettings>>,
    by: HashMap<String, Http>,
}

fn build(settings: &ServerSettings) -> HashMap<String, Http> {
    let Some(web) = &settings.web else {
        return HashMap::new();
    };
    settings
        .services
        .iter()
        .filter_map(|(name, svc)| {
            let http = svc.health.as_ref()?.http.as_ref()?;
            let site = web.site_of(name).filter(|s| s.tls != WebTls::Passthrough)?;
            let ask = Ask {
                host: &http.host,
                path: &http.path,
                agent: AGENT,
            };
            let check = HealthCheck::build(&site_options(&site.options), &ask);
            Some((name.clone(), Arc::new(check)))
        })
        .collect()
}

impl Checks {
    pub(in crate::server) fn new() -> Checks {
        Checks::default()
    }

    pub(in crate::server) fn refresh(&mut self, settings: &Arc<ServerSettings>) {
        if self.of.as_ref().is_some_and(|s| Arc::ptr_eq(s, settings)) {
            return;
        }
        self.by = build(settings);
        self.of = Some(settings.clone());
    }

    pub(in crate::server) fn of(&self, service: &str) -> Option<Http> {
        self.by.get(service).cloned()
    }
}

pub(in crate::server) async fn exchange(
    ch: Channel,
    plaintext: bool,
    http: &Http,
) -> Result<(), String> {
    let check = match http.as_ref() {
        Ok(c) => c,
        Err(e) => return Err(e.clone()),
    };
    let (ours, theirs) = burrow_web::pipe();
    ours.set_reset(true);
    let mut pump = Box::pin(pump_tcp(ch, Incoming::Web(theirs), plaintext));
    let run = check.run(Box::new(ours));
    tokio::pin!(run);
    tokio::select! {
        r = &mut run => {
            tokio::spawn(pump);
            r
        }
        _ = &mut pump => run.await,
    }
}
