mod data;

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, watch};
use tracing::{debug, error, info, warn};

use crate::config::settings::ClientSettings;
use crate::failure::io_reason;
use crate::handshake::{Channel, ClientHandshakeError, initiate};
use crate::net::{dial, tune};
use crate::protocol::{
    ByeReason, ChannelKind, ClientMsg, Hello, Preamble, ServerMsg, ServiceInfo, TOKEN_LEN, VERSION,
};

const STABLE_AFTER: Duration = Duration::from_secs(10);
const MAX_OPENING: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Connecting,
    Connected,
    Rejected,
}

pub struct Backoff {
    initial: Duration,
    max: Duration,
    current: Duration,
}

impl Backoff {
    pub fn new(initial: Duration, max: Duration) -> Self {
        Backoff {
            initial,
            max,
            current: initial,
        }
    }

    pub fn reset(&mut self) {
        self.current = self.initial;
    }

    pub fn next_delay(&mut self) -> Duration {
        let base = self.current;
        self.current = (self.current * 2).min(self.max);
        let mut b = [0u8; 4];
        getrandom::fill(&mut b).expect("operating system random source is available");
        let frac = f64::from(u32::from_le_bytes(b)) / f64::from(u32::MAX);
        base.mul_f64(0.5 + frac * 0.5)
    }
}

pub struct Client {
    settings: Arc<ClientSettings>,
    state: watch::Sender<State>,
    halt: watch::Sender<bool>,
}

pub enum SessionEnd {
    Lost(String),
    Rejected,
    Removed,
    Stop,
}

impl Client {
    pub fn new(settings: ClientSettings) -> Self {
        let (state, _) = watch::channel(State::Connecting);
        Client {
            settings: Arc::new(settings),
            state,
            halt: watch::Sender::new(false),
        }
    }

    pub fn state(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }

    fn preamble(&self, kind: ChannelKind) -> Preamble {
        Preamble {
            kind,
            cipher: self.settings.cipher,
            plaintext: self.settings.insecure_plaintext,
            name: self.settings.name.clone(),
        }
    }

    pub async fn run(self, mut stop: watch::Receiver<bool>) {
        let s = self.settings.clone();
        if s.insecure_plaintext {
            warn!("insecure_plaintext is enabled: forwarded TCP data is NOT encrypted");
        }
        let mut backoff = Backoff::new(s.reconnect_initial, s.reconnect_max);
        loop {
            let started = Instant::now();
            let end = tokio::select! {
                e = self.session() => e,
                _ = stop.changed() => SessionEnd::Stop,
            };
            if started.elapsed() > STABLE_AFTER {
                backoff.reset();
            }
            let delay = backoff.next_delay();
            match end {
                SessionEnd::Stop => {
                    info!("client stopped");
                    self.halt.send_replace(true);
                    return;
                }
                SessionEnd::Lost(why) => {
                    let _ = self.state.send(State::Connecting);
                    warn!("{why}; retrying in {:.1}s", delay.as_secs_f64());
                }
                SessionEnd::Rejected => {
                    let _ = self.state.send(State::Rejected);
                    error!(
                        "handshake rejected by {}: wrong secret or unknown client name \"{}\" (check both against the server config, or regenerate with: burrow client-config {} -c server.yaml --server {}); retrying in {:.1}s",
                        s.server,
                        s.name,
                        s.name,
                        s.server,
                        delay.as_secs_f64()
                    );
                }
                SessionEnd::Removed => {
                    let _ = self.state.send(State::Rejected);
                    error!(
                        "the server removed client \"{}\" from its config; retrying in {:.1}s",
                        s.name,
                        delay.as_secs_f64()
                    );
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = stop.changed() => {
                    info!("client stopped");
                    self.halt.send_replace(true);
                    return;
                }
            }
        }
    }

    async fn open(&self, kind: ChannelKind, hello: Hello) -> Result<Channel, SessionEnd> {
        let s = &self.settings;
        let stream = dial(&s.server, s.proxy.as_ref(), s.connect_timeout)
            .await
            .map_err(|e| {
                let via = s
                    .proxy
                    .as_ref()
                    .map(|p| format!(" via proxy {p:?}"))
                    .unwrap_or_default();
                SessionEnd::Lost(format!(
                    "server unreachable at {}{via} ({})",
                    s.server,
                    io_reason(&e)
                ))
            })?;
        tune(&stream, &s.tcp);
        let preamble = self.preamble(kind);
        let fut = initiate(stream, &preamble, &s.secret, &hello);
        match tokio::time::timeout(s.connect_timeout, fut).await {
            Ok(Ok(ch)) => Ok(ch),
            Ok(Err(ClientHandshakeError::Rejected)) => Err(SessionEnd::Rejected),
            Ok(Err(ClientHandshakeError::Io(e))) => Err(SessionEnd::Lost(format!(
                "handshake with {} failed ({})",
                s.server,
                io_reason(&e)
            ))),
            Err(_) => Err(SessionEnd::Lost(format!(
                "handshake with {} timed out",
                s.server
            ))),
        }
    }

    async fn session(&self) -> SessionEnd {
        let s = self.settings.clone();
        let ch = match self
            .open(ChannelKind::Control, Hello::Control { version: VERSION })
            .await
        {
            Ok(c) => c,
            Err(e) => return e,
        };
        let Channel {
            stream,
            mut writer,
            mut reader,
        } = ch;
        let (mut rd, mut wr) = stream.into_split();

        let (token, heartbeat) =
            match tokio::time::timeout(s.connect_timeout, reader.recv(&mut rd)).await {
                Ok(Ok(Some(b))) => match ServerMsg::decode(b) {
                    Ok(ServerMsg::Welcome {
                        token,
                        heartbeat_secs,
                    }) => (token, Duration::from_secs(heartbeat_secs.max(1).into())),
                    Ok(ServerMsg::Bye { reason }) => return bye(reason),
                    _ => return SessionEnd::Lost("server sent an unexpected first message".into()),
                },
                Ok(Ok(None)) => return SessionEnd::Rejected,
                Ok(Err(e)) => {
                    return SessionEnd::Lost(format!(
                        "connection to {} failed ({})",
                        s.server,
                        io_reason(&e)
                    ));
                }
                Err(_) => {
                    return SessionEnd::Lost(format!("server {} did not answer in time", s.server));
                }
            };
        let _ = self.state.send(State::Connected);
        info!("connected to {} as \"{}\"", s.server, s.name);

        let opening = Arc::new(Semaphore::new(MAX_OPENING));
        let timeout = heartbeat * 3;
        loop {
            let msg = match tokio::time::timeout(timeout, reader.recv(&mut rd)).await {
                Ok(Ok(Some(b))) => match ServerMsg::decode(b) {
                    Ok(m) => m,
                    Err(_) => return SessionEnd::Lost("server sent an invalid message".into()),
                },
                Ok(Ok(None)) => {
                    return SessionEnd::Lost(format!("server {} closed the connection", s.server));
                }
                Ok(Err(e)) => {
                    return SessionEnd::Lost(format!(
                        "connection to {} lost ({})",
                        s.server,
                        io_reason(&e)
                    ));
                }
                Err(_) => {
                    return SessionEnd::Lost(format!(
                        "no heartbeat from {} for {}s",
                        s.server,
                        timeout.as_secs()
                    ));
                }
            };
            match msg {
                ServerMsg::Ping => {
                    if writer
                        .send(&mut wr, &ClientMsg::Pong.encode())
                        .await
                        .is_err()
                    {
                        return SessionEnd::Lost(format!("connection to {} lost", s.server));
                    }
                }
                ServerMsg::OpenData { count } => {
                    for _ in 0..count {
                        self.spawn_data(token, opening.clone());
                    }
                }
                ServerMsg::Services(list) => self.report_services(&list),
                ServerMsg::Bye { reason } => return bye(reason),
                ServerMsg::Welcome { .. } => {}
            }
        }
    }

    fn report_services(&self, list: &[ServiceInfo]) {
        if list.is_empty() {
            info!("the server exposes no services for this client yet");
        }
        for svc in list {
            info!(
                "service \"{}\": {} server port {} -> {}",
                svc.name,
                svc.protocol.as_str(),
                svc.bind_port,
                svc.target
            );
            data::precheck(&self.settings, svc);
        }
    }

    fn spawn_data(&self, token: [u8; TOKEN_LEN], opening: Arc<Semaphore>) {
        let me = Client {
            settings: self.settings.clone(),
            state: self.state.clone(),
            halt: self.halt.clone(),
        };
        let mut halted = self.halt.subscribe();
        tokio::spawn(async move {
            tokio::select! {
                _ = me.data_channel(token, opening) => {}
                _ = halted.wait_for(|h| *h) => {}
            }
        });
    }

    async fn data_channel(&self, token: [u8; TOKEN_LEN], opening: Arc<Semaphore>) {
        let permit = opening.acquire_owned().await;
        let hello = Hello::Data {
            version: VERSION,
            token,
        };
        let ch = self.open(ChannelKind::Data, hello).await;
        drop(permit);
        match ch {
            Ok(ch) => data::serve(self.settings.clone(), ch).await,
            Err(SessionEnd::Lost(why)) => debug!("data channel: {why}"),
            Err(_) => debug!("data channel rejected"),
        }
    }
}

fn bye(reason: ByeReason) -> SessionEnd {
    match reason {
        ByeReason::Shutdown => SessionEnd::Lost("server is shutting down".into()),
        ByeReason::Replaced => SessionEnd::Lost(
            "another client connected with the same name and replaced this one".into(),
        ),
        ByeReason::Removed => SessionEnd::Removed,
        ByeReason::Timeout => SessionEnd::Lost("server timed out this connection".into()),
        ByeReason::PlaintextMismatch => SessionEnd::Lost(
            "server and client disagree on insecure_plaintext (set it the same on both sides)"
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_with_jitter_and_caps() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(1));
        let d: Vec<Duration> = (0..8).map(|_| b.next_delay()).collect();
        assert!(d[0] >= Duration::from_millis(50) && d[0] <= Duration::from_millis(100));
        assert!(d[3] >= Duration::from_millis(400) && d[3] <= Duration::from_millis(800));
        assert!(d.iter().all(|x| *x <= Duration::from_secs(1)));
        b.reset();
        assert!(b.next_delay() <= Duration::from_millis(100));
    }
}
