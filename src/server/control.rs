use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{ClientSession, Shared, TICK};
use crate::handshake::Accepted;
use crate::protocol::{ByeReason, ServerMsg, TOKEN_LEN};

pub(super) async fn control_session(shared: Arc<Shared>, acc: Accepted, peer: SocketAddr) {
    let Accepted {
        preamble, channel, ..
    } = acc;
    let name = preamble.name.clone();
    let settings = shared.settings();
    let (stream, mut writer, mut reader) = (channel.stream, channel.writer, channel.reader);
    let (mut rd, mut wr) = stream.into_split();

    if preamble.plaintext != settings.insecure_plaintext {
        warn!(
            "client \"{name}\" from {peer} has insecure_plaintext {} but the server has it {}; disconnecting",
            on_off(preamble.plaintext),
            on_off(settings.insecure_plaintext)
        );
        let _ = writer
            .send(
                &mut wr,
                &ServerMsg::Bye {
                    reason: ByeReason::PlaintextMismatch,
                }
                .encode(),
            )
            .await;
        return;
    }

    let mut token = [0u8; TOKEN_LEN];
    getrandom::fill(&mut token).expect("operating system random source is available");
    let (tx, mut rx) = mpsc::channel::<ServerMsg>(256);
    let session = Arc::new(ClientSession::new(
        name.clone(),
        token,
        tx,
        &Shared::pool_config(&settings),
    ));
    let old = shared
        .sessions
        .lock()
        .expect("sessions lock")
        .insert(name.clone(), session.clone());
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
    let services = ServerMsg::Services(Shared::services_for(&settings, &name));
    let mut stop = shared.shutdown.clone();
    let writer_session = session.clone();
    let writer_task = tokio::spawn(async move {
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
    });

    let timeout = interval * 3;
    let reason = loop {
        tokio::select! {
            r = tokio::time::timeout(timeout, reader.recv(&mut rd)) => match r {
                Ok(Ok(Some(_))) => continue,
                Ok(Ok(None)) => break None,
                Ok(Err(e)) => break Some(format!("connection error: {e}")),
                Err(_) => break Some(format!("no heartbeat for {}s", timeout.as_secs())),
            },
            _ = stop.changed() => break Some("server shutting down".to_string()),
        }
    };
    let current = {
        let mut sessions = shared.sessions.lock().expect("sessions lock");
        let current = sessions
            .get(&name)
            .is_some_and(|s| Arc::ptr_eq(s, &session));
        if current {
            sessions.remove(&name);
        }
        current
    };
    session.close();
    if current {
        match reason {
            Some(reason) => info!("client \"{name}\" disconnected ({reason})"),
            None => info!("client \"{name}\" disconnected"),
        }
    }
    let abort = writer_task.abort_handle();
    if tokio::time::timeout(Duration::from_secs(1), writer_task)
        .await
        .is_err()
    {
        abort.abort();
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}
