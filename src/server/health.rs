use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

use super::Shared;
use crate::config::model::Protocol;
use crate::config::settings::Health;
use crate::protocol::{Connect, Status};

const TICK: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub(crate) struct Probe {
    up: bool,
    fails: u32,
    last: Option<Instant>,
    running: bool,
}

impl Default for Probe {
    fn default() -> Self {
        Probe {
            up: true,
            fails: 0,
            last: None,
            running: false,
        }
    }
}

pub(crate) type Probes = HashMap<(String, String), Probe>;

impl Shared {
    pub(crate) fn is_up(&self, service: &str, client: &str) -> bool {
        self.probes
            .lock()
            .expect("probes lock")
            .get(&(service.to_string(), client.to_string()))
            .is_none_or(|p| p.up)
    }

    fn record(&self, service: &str, client: &str, health: Health, result: Result<(), String>) {
        let mut probes = self.probes.lock().expect("probes lock");
        let Some(p) = probes.get_mut(&(service.to_string(), client.to_string())) else {
            return;
        };
        p.running = false;
        match result {
            Ok(()) => {
                p.fails = 0;
                if !p.up {
                    p.up = true;
                    info!("service \"{service}\": client \"{client}\" reaches its target again");
                }
            }
            Err(why) => {
                p.fails += 1;
                if p.up && p.fails >= health.failures {
                    p.up = false;
                    warn!(
                        "service \"{service}\": client \"{client}\" is down ({why}); new connections go to the other clients"
                    );
                }
            }
        }
    }
}

pub(super) async fn run(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        let settings = shared.settings();
        let connected = shared.session_names();
        let mut due = Vec::new();
        {
            let mut probes = shared.probes.lock().expect("probes lock");
            probes.retain(|(service, client), _| {
                settings
                    .services
                    .get(service)
                    .is_some_and(|s| s.health.is_some() && s.clients.contains(client))
            });
            for (name, svc) in &settings.services {
                let Some(health) = svc.health else { continue };
                for client in svc.clients.iter().filter(|c| connected.contains(c)) {
                    let p = probes.entry((name.clone(), client.clone())).or_default();
                    if !p.running && p.last.is_none_or(|t| t.elapsed() >= health.interval) {
                        p.running = true;
                        p.last = Some(Instant::now());
                        due.push((name.clone(), client.clone(), svc.target_at(0), health));
                    }
                }
            }
        }
        for (service, client, target, health) in due {
            let shared = shared.clone();
            tokio::spawn(async move {
                let result = tokio::time::timeout(
                    health.timeout,
                    probe(&shared, &service, &client, &target),
                )
                .await
                .unwrap_or_else(|_| Err(format!("no answer within {}s", health.timeout.as_secs())));
                shared.record(&service, &client, health, result);
            });
        }
    }
}

async fn probe(shared: &Shared, service: &str, client: &str, target: &str) -> Result<(), String> {
    let session = shared
        .session(client)
        .ok_or_else(|| "the client disconnected".to_string())?;
    let mut ch = session
        .take()
        .await
        .map_err(|_| "the client has no free data channel".to_string())?;
    let connect = Connect {
        protocol: Protocol::Tcp,
        service: service.to_string(),
        target: target.to_string(),
    };
    ch.writer
        .send(&mut ch.stream, &connect.encode())
        .await
        .map_err(|e| format!("cannot reach the client: {e}"))?;
    let settings = shared.settings();
    let raw = settings.insecure_plaintext && ch.stream.supports_half_close();
    if let Some(version) = settings
        .services
        .get(service)
        .and_then(|s| s.proxy_protocol)
    {
        let header = crate::proxy_protocol::local(version);
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
    if raw {
        let _ = ch.stream.shutdown().await;
    } else {
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
