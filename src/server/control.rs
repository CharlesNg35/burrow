use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{ClientSession, Shared, TICK};
use crate::wire::handshake::Accepted;
use crate::wire::protocol::{ByeReason, ClientMsg, ServerMsg, TOKEN_LEN};
use crate::wire::record::RecordWriter;
use crate::wire::transport::OwnedWriteHalf;

pub(super) async fn control_session(
    shared: Arc<Shared>,
    acc: Accepted,
    peer: SocketAddr,
    instance: [u8; TOKEN_LEN],
) {
    let Accepted {
        preamble,
        channel,
        secret,
        ..
    } = acc;
    let name = preamble.name.clone();
    let settings = shared.settings();
    let (stream, mut writer, mut reader) = (channel.stream, channel.writer, channel.reader);
    let over_websocket = stream.is_websocket();
    let (mut rd, mut wr) = stream.into_split();

    if preamble.plaintext != settings.insecure_plaintext {
        warn!(
            "client \"{name}\" from {peer} has insecure_plaintext {} but the server has it {}; disconnecting",
            on_off(preamble.plaintext),
            on_off(settings.insecure_plaintext)
        );
        say_bye(&mut writer, &mut wr, ByeReason::PlaintextMismatch).await;
        return;
    }

    let mut token = [0u8; TOKEN_LEN];
    getrandom::fill(&mut token).expect("operating system random source is available");
    let (tx, rx) = mpsc::channel::<ServerMsg>(256);
    let mut session = ClientSession::new(
        name.clone(),
        peer,
        token,
        tx,
        secret,
        instance,
        &Shared::pool_config(&settings),
    );
    session.over_websocket = over_websocket;
    let session = Arc::new(session);
    let taken = take_seat(&shared, &name, &session, instance, preamble.plaintext);
    let old = match taken {
        Ok(old) => old,
        Err(None) => {
            info!(
                "client \"{name}\" from {peer} refused: its secret was removed while it connected"
            );
            say_bye(&mut writer, &mut wr, ByeReason::Removed).await;
            return;
        }
        Err(Some(None)) => {
            info!(
                "client \"{name}\" from {peer} refused: insecure_plaintext changed while it connected"
            );
            say_bye(&mut writer, &mut wr, ByeReason::PlaintextMismatch).await;
            return;
        }
        Err(Some(Some(other))) => {
            static TWICE: crate::policy::throttle::Throttle =
                crate::policy::throttle::Throttle::new();
            if let Some(more) = TWICE.allow() {
                warn!(
                    "client \"{name}\" from {peer} refused: a client with that name is already connected from {other}; two machines may be using the same config{more}"
                );
            }
            say_bye(&mut writer, &mut wr, ByeReason::Duplicate).await;
            return;
        }
    };
    if let Some(old) = old {
        warn!("client \"{name}\" connected again from {peer}; closing its previous connection");
        let _ = old.control.try_send(ServerMsg::Bye {
            reason: ByeReason::Replaced,
        });
        old.close();
    }
    info!("client \"{name}\" connected from {peer}");

    let interval = settings.heartbeat_interval;
    let welcome = ServerMsg::Welcome {
        token,
        heartbeat_secs: interval.as_secs().clamp(1, u16::MAX as u64) as u16,
    };
    let services = ServerMsg::Services(settings.services_for(&name));
    let mut stop = shared.shutdown.clone();
    let writer_session = session.clone();
    let writer_task = tokio::spawn(write_loop(
        writer,
        wr,
        rx,
        writer_session,
        welcome,
        services,
        interval,
    ));

    let timeout = crate::wire::protocol::peer_timeout(interval);
    let mut peer_left = false;
    let reason = loop {
        tokio::select! {
            r = tokio::time::timeout(timeout, reader.recv(&mut rd)) => match r {
                Ok(Ok(Some(b))) => {
                    if let Ok(ClientMsg::Allow(rules)) = ClientMsg::decode(b) {
                        note_refused_targets(&shared, &name, &rules);
                        session.set_allow(rules);
                    }
                    continue;
                }
                Ok(Ok(None)) => {
                    peer_left = true;
                    break None;
                }
                Ok(Err(e)) => break Some(format!("connection error: {e}")),
                Err(_) => break Some(format!("no heartbeat for {}s", timeout.as_secs())),
            },
            _ = stop.changed() => break Some("server shutting down".to_string()),
            _ = session.closed() => break None,
        }
    };
    let current = leave_seat(&shared, &name, &session);
    session.close();
    if current {
        shared.metrics.client_left(&name);
        match reason {
            Some(reason) => info!("client \"{name}\" disconnected ({reason})"),
            None => info!("client \"{name}\" disconnected"),
        }
    }
    let abort = writer_task.abort_handle();
    if peer_left {
        abort.abort();
    }
    if tokio::time::timeout(Duration::from_secs(1), writer_task)
        .await
        .is_err()
    {
        abort.abort();
    }
}

async fn say_bye(writer: &mut RecordWriter, wr: &mut OwnedWriteHalf, reason: ByeReason) {
    let _ = writer.send(wr, &ServerMsg::Bye { reason }.encode()).await;
}

type Seat = Result<Option<Arc<ClientSession>>, Option<Option<SocketAddr>>>;

fn take_seat(
    shared: &Shared,
    name: &str,
    session: &Arc<ClientSession>,
    instance: [u8; TOKEN_LEN],
    plaintext: bool,
) -> Seat {
    let mut sessions = shared.sessions.lock().expect("sessions lock");
    if !shared.allows(name, &session.secret) {
        Err(None)
    } else if shared.settings().insecure_plaintext != plaintext {
        Err(Some(None))
    } else {
        match sessions.get(name) {
            Some(other) if other.instance != instance => Err(Some(Some(other.peer))),
            _ => Ok(sessions.insert(name.to_string(), session.clone())),
        }
    }
}

async fn write_loop(
    mut writer: RecordWriter,
    mut wr: OwnedWriteHalf,
    mut rx: mpsc::Receiver<ServerMsg>,
    writer_session: Arc<ClientSession>,
    welcome: ServerMsg,
    services: ServerMsg,
    interval: Duration,
) {
    if writer.send(&mut wr, &welcome.encode()).await.is_err()
        || writer.send(&mut wr, &services.encode()).await.is_err()
    {
        return;
    }
    writer_session.start();
    let mut ping = tokio::time::interval(interval);
    let mut tick = tokio::time::interval(TICK);
    loop {
        let msg = tokio::select! {
            m = rx.recv() => match m {
                Some(m) => m,
                None => return,
            },
            _ = ping.tick() => ServerMsg::Ping,
            _ = tick.tick() => {
                writer_session.tick();
                continue;
            }
        };
        let bye = matches!(msg, ServerMsg::Bye { .. });
        if writer.send(&mut wr, &msg.encode()).await.is_err() || bye {
            return;
        }
    }
}

fn leave_seat(shared: &Shared, name: &str, session: &Arc<ClientSession>) -> bool {
    let mut sessions = shared.sessions.lock().expect("sessions lock");
    let current = sessions.get(name).is_some_and(|s| Arc::ptr_eq(s, session));
    if current {
        sessions.remove(name);
    }
    current
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn refused_targets(
    settings: &crate::config::settings::ServerSettings,
    client: &str,
    allow: &crate::policy::allowlist::AllowList,
) -> Vec<String> {
    let refuses = |t: &str| match crate::config::check::unix::path_of(t) {
        Some(path) => !allow.permits_socket(path),
        None => t
            .parse::<SocketAddr>()
            .is_ok_and(|addr| !allow.permits(addr)),
    };
    let mut out: Vec<String> = Vec::new();
    for (name, svc) in &settings.services {
        for e in svc.entries().iter().filter(|e| e.client == client) {
            let last = svc.ports.max(1) - 1;
            let ends = [svc.entry_target_at(e, 0), svc.entry_target_at(e, last)];
            let line = format!("\"{name}\" ({})", svc.target_of(e));
            if ends.iter().any(|t| refuses(t)) && !out.contains(&line) {
                out.push(line);
            }
        }
    }
    out
}

fn note_refused_targets(shared: &Shared, client: &str, rules: &[String]) {
    use crate::policy::allowlist::{AllowList, Rule};
    let allow = if rules == [burrow_syntax::rule::ANY] {
        AllowList::All
    } else {
        let parsed: Option<Vec<Rule>> = rules.iter().map(|r| r.parse().ok()).collect();
        let Some(parsed) = parsed else {
            return;
        };
        AllowList::Rules(parsed)
    };
    let refused = refused_targets(&shared.settings(), client, &allow);
    if refused.is_empty() {
        return;
    }
    static REFUSED: crate::policy::throttle::Throttle = crate::policy::throttle::Throttle::new();
    if let Some(more) = REFUSED.allow() {
        warn!(
            "client \"{client}\" does not allow the target of service {}, so it will refuse those connections; add the target to \"allow\" in the config of client \"{client}\"{more}",
            refused.join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::allowlist::{AllowList, Rule};

    fn settings() -> crate::config::settings::ServerSettings {
        let secret = || crate::wire::secret::Secret::generate().encode();
        let yaml = format!(
            "listen: 127.0.0.1:7835\nclients:\n  gw:\n    secret: {}\n  home:\n    secret: {}\nservices:\n  app:\n    clients: [home, {{name: gw, target: \"10.0.0.5:80\"}}]\n    bind: 4000\n    target: \"10.0.0.1:80\"\n",
            secret(),
            secret()
        );
        let o = crate::config::load_str(
            "server.yaml",
            &yaml,
            crate::config::Format::Yaml,
            Some(crate::config::Role::Server),
            &|_: &str| None,
        );
        match o.loaded {
            Some(crate::config::Loaded::Server(s, _)) => *s,
            _ => panic!("{:#?}", o.report.diagnostics),
        }
    }

    fn only(rule: &str) -> AllowList {
        AllowList::Rules(vec![rule.parse::<Rule>().unwrap()])
    }

    #[test]
    fn refused_targets_follow_each_entrys_own_target() {
        let s = settings();
        assert!(refused_targets(&s, "gw", &only("10.0.0.5/32")).is_empty());
        assert_eq!(
            refused_targets(&s, "gw", &only("10.0.0.1/32")),
            ["\"app\" (10.0.0.5:80)"]
        );
        assert_eq!(
            refused_targets(&s, "home", &only("10.0.0.5/32")),
            ["\"app\" (10.0.0.1:80)"]
        );
        assert!(refused_targets(&s, "home", &only("10.0.0.1/32")).is_empty());
    }
}
