use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

use super::Shared;
use super::web::{Checks, Http, exchange};
use crate::config::settings::{EntryKey, HttpCheck};
use crate::wire::protocol::{Connect, Status};
use burrow_config::Protocol;

const TICK: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub(crate) struct Probe {
    up: bool,
    fails: u32,
    last: Option<Instant>,
    running: bool,
    reason: Option<String>,
    http: Option<HttpCheck>,
}

impl Default for Probe {
    fn default() -> Self {
        Probe {
            up: true,
            fails: 0,
            last: None,
            running: false,
            reason: None,
            http: None,
        }
    }
}

pub(crate) type Probes = HashMap<String, HashMap<EntryKey, Probe>>;

fn who(client: &str, target: &str, svc: Option<&crate::config::settings::Service>) -> String {
    match svc.filter(|s| s.target != target) {
        Some(_) => format!("client \"{client}\" ({target})"),
        None => format!("client \"{client}\""),
    }
}

impl Shared {
    pub(crate) fn is_up(&self, service: &str, key: &EntryKey) -> bool {
        self.probes
            .lock()
            .expect("probes lock")
            .get(service)
            .and_then(|of| of.get(key))
            .is_none_or(|p| p.up)
    }

    pub(crate) fn health_reason(&self, service: &str, key: &EntryKey) -> Option<String> {
        self.probes
            .lock()
            .expect("probes lock")
            .get(service)
            .and_then(|of| of.get(key))
            .and_then(|p| p.reason.clone())
    }

    fn record(&self, service: &str, key: &EntryKey, failures: u32, result: Result<(), String>) {
        let settings = self.settings();
        let who = who(&key.client, &key.base, settings.services.get(service));
        let mut probes = self.probes.lock().expect("probes lock");
        let Some(p) = probes.get_mut(service).and_then(|of| of.get_mut(key)) else {
            return;
        };
        p.running = false;
        match result {
            Ok(()) => {
                p.fails = 0;
                p.reason = None;
                if !p.up {
                    p.up = true;
                    info!("service \"{service}\": {who} reaches its target again");
                }
            }
            Err(why) => {
                p.fails += 1;
                if p.up && p.fails >= failures {
                    p.up = false;
                    warn!(
                        "service \"{service}\": {who} is down ({why}); new connections go to the other clients"
                    );
                }
                p.reason = Some(why);
            }
        }
    }
}

pub(super) async fn run(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(TICK);
    let mut checks = Checks::new();
    loop {
        tick.tick().await;
        let settings = shared.settings();
        checks.refresh(&settings);
        let connected: Vec<String> = settings
            .services
            .values()
            .filter(|s| s.health.is_some())
            .flat_map(|s| shared.connected(&s.clients))
            .map(|v| v.name().to_string())
            .collect();
        let mut due = Vec::new();
        {
            let mut probes = shared.probes.lock().expect("probes lock");
            probes.retain(|service, of| {
                let Some(s) = settings.services.get(service) else {
                    return false;
                };
                let entries = s.entries();
                of.retain(|key, p| {
                    entries.iter().any(|e| &e.key == key)
                        && s.health.as_ref().is_some_and(|h| h.http == p.http)
                });
                !of.is_empty()
            });
            for (name, svc) in &settings.services {
                let Some(health) = &svc.health else { continue };
                for entry in svc
                    .entries()
                    .iter()
                    .filter(|e| connected.contains(&e.client))
                {
                    let p = probes
                        .entry(name.clone())
                        .or_default()
                        .entry(entry.key.clone())
                        .or_insert_with(|| Probe {
                            http: health.http.clone(),
                            ..Probe::default()
                        });
                    if !p.running && p.last.is_none_or(|t| t.elapsed() >= health.interval) {
                        p.running = true;
                        p.last = Some(Instant::now());
                        let check = Check {
                            key: entry.key.clone(),
                            target: svc.entry_target_at(entry, 0),
                            http: checks.of(name),
                            timeout: health.timeout,
                            failures: health.failures,
                        };
                        due.push((name.clone(), check));
                    }
                }
            }
        }
        for (service, check) in due {
            let client = check.key.client.to_string();
            let shared = shared.clone();
            tokio::spawn(async move {
                let result =
                    tokio::time::timeout(check.timeout, probe(&shared, &service, &client, &check))
                        .await
                        .unwrap_or_else(|_| {
                            Err(format!("no answer within {}s", check.timeout.as_secs()))
                        });
                shared.record(&service, &check.key, check.failures, result);
            });
        }
    }
}

struct Check {
    key: EntryKey,
    target: String,
    http: Option<Http>,
    timeout: Duration,
    failures: u32,
}

async fn probe(shared: &Shared, service: &str, client: &str, check: &Check) -> Result<(), String> {
    let target = check.target.as_str();
    let session = shared
        .via(client)
        .ok_or_else(|| "the client disconnected".to_string())?;
    let mut ch = session
        .take()
        .await
        .map_err(|_| "the client has no free data channel".to_string())?;
    let settings = shared.settings();
    let raw = settings.insecure_plaintext && ch.stream.supports_half_close();
    if raw {
        ch.stream.reset_on_close();
    }
    let connect = Connect {
        protocol: Protocol::Tcp,
        service: service.to_string(),
        target: target.to_string(),
        only: None,
    };
    ch.writer
        .send(&mut ch.stream, &connect.encode())
        .await
        .map_err(|e| format!("cannot reach the client: {e}"))?;
    if let Some(version) = settings
        .services
        .get(service)
        .and_then(|s| s.proxy_protocol)
    {
        let header = crate::wire::proxy_protocol::local(version);
        let sent = if raw {
            ch.stream.write_all(&header).await
        } else {
            ch.writer.send(&mut ch.stream, &header).await
        };
        sent.map_err(|e| format!("cannot reach the client: {e}"))?;
    }
    let answer = match ch.reader.recv(&mut ch.stream).await {
        Ok(Some(b)) => Status::decode(b).map_err(|_| "invalid reply".to_string())?,
        _ => return Err("the client closed the check".into()),
    };
    if answer == Status::Ok {
        shared.accepted(service, client, target);
        if let Some(http) = &check.http {
            return exchange(ch, settings.insecure_plaintext, http).await;
        }
    }
    if !raw {
        let _ = ch.writer.send(&mut ch.stream, &[]).await;
        let _ = ch.stream.flush().await;
        let _ = ch.stream.finish().await;
    }
    match answer {
        Status::Ok => Ok(()),
        Status::Unreachable => Err(format!("cannot connect to {target}")),
        Status::Denied => Err(format!("{target} is not in its allow list")),
        Status::Unresolved => Err(format!("cannot resolve {target}")),
        other => Err(format!("answered {other:?}")),
    }
}
