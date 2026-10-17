mod data;
pub mod reconnect;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, watch};
use tracing::{debug, error, info, warn};

use crate::config::settings::ClientSettings;
use crate::dialer::{DialError, Dialer};
use crate::failure::io_reason;
use crate::handshake::Channel;
use crate::protocol::{
    ByeReason, ChannelKind, ClientMsg, Hello, ServerMsg, ServiceInfo, TOKEN_LEN, VERSION,
};
use crate::transport::resolve::Resolver;
use reconnect::{Backoff, Outage, Report, human};

const STABLE_AFTER: Duration = Duration::from_secs(10);
const REJECT_CONFIRM_ATTEMPTS: u32 = 3;
const MAX_OPENING: usize = 32;
const DATA_PATIENCE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Connecting,
    Connected,
    Rejected,
}

pub struct Client {
    settings: Arc<ClientSettings>,
    state: watch::Sender<State>,
    halt: watch::Sender<bool>,
    dialer: Arc<Dialer>,
    outage: Arc<Mutex<Outage>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    Stopped,
    GaveUp(Duration),
    Rejected,
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
            state,
            halt: watch::Sender::new(false),
            dialer: Arc::new(Dialer::client(&settings)),
            settings: Arc::new(settings),
            outage: Arc::new(Mutex::new(Outage::default())),
        }
    }

    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        Arc::get_mut(&mut self.dialer)
            .expect("set before the client runs")
            .resolver = resolver;
        self
    }

    pub fn state(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }

    pub async fn run(self, mut stop: watch::Receiver<bool>) -> RunEnd {
        let s = self.settings.clone();
        if s.insecure_plaintext {
            warn!("insecure_plaintext is enabled: forwarded TCP data is NOT encrypted");
        }
        let mut backoff = Backoff::new(s.reconnect_initial, s.reconnect_max);
        let mut rejected: Option<(Instant, u32)> = None;
        let end = loop {
            let started = Instant::now();
            let end = tokio::select! {
                e = self.session() => e,
                _ = stop.changed() => SessionEnd::Stop,
            };
            if started.elapsed() > STABLE_AFTER {
                backoff.reset();
            }
            let delay = backoff.next_delay();
            if !matches!(end, SessionEnd::Rejected) {
                rejected = None;
            }
            let why = match end {
                SessionEnd::Stop => break RunEnd::Stopped,
                SessionEnd::Lost(why) => {
                    let _ = self.state.send(State::Connecting);
                    why
                }
                SessionEnd::Rejected => {
                    let now = Instant::now();
                    let (since, count) = rejected.get_or_insert((now, 0));
                    *count += 1;
                    let confirmed = *count >= REJECT_CONFIRM_ATTEMPTS
                        && now.duration_since(*since) >= s.reject_confirm;
                    if confirmed {
                        let _ = self.state.send(State::Rejected);
                    } else {
                        let _ = self.state.send(State::Connecting);
                    }
                    let why = format!(
                        "handshake rejected by {}: wrong secret or unknown client name \"{}\", or the server is busy or restarting (check both against the server config, or print a new client config on the server with: burrow client-config {} -o client.yaml)",
                        s.server, s.name, s.name
                    );
                    if confirmed && s.exit_on_rejected {
                        error!("{why}; not retrying (reconnect.on_rejected: exit)");
                        break RunEnd::Rejected;
                    }
                    why
                }
                SessionEnd::Removed => {
                    let _ = self.state.send(State::Rejected);
                    let why = format!("the server removed client \"{}\" from its config", s.name);
                    if s.exit_on_rejected {
                        error!("{why}; not retrying (reconnect.on_rejected: exit)");
                        break RunEnd::Rejected;
                    }
                    why
                }
            };
            let now = Instant::now();
            let report = self.outage.lock().expect("outage lock").failed(now);
            match report {
                Report::First => warn!(
                    "{why}; retrying in {:.1}s, and will keep retrying",
                    delay.as_secs_f64()
                ),
                Report::Summary { down_for, attempts } => warn!(
                    "still cannot reach {} after {} ({attempts} attempts); retrying every {}s at most. last error: {why}",
                    s.server,
                    human(down_for),
                    s.reconnect_max.as_secs()
                ),
                Report::Quiet => debug!("{why}; retrying in {:.1}s", delay.as_secs_f64()),
            }
            if let Some(limit) = s.give_up_after {
                let down = self.outage.lock().expect("outage lock").down_for(now);
                if down >= limit {
                    error!(
                        "giving up: {} has been unreachable for {} (reconnect.give_up_after_secs: {})",
                        s.server,
                        human(down),
                        limit.as_secs()
                    );
                    break RunEnd::GaveUp(down);
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = stop.changed() => break RunEnd::Stopped,
            }
        };
        if end == RunEnd::Stopped {
            info!("client stopped");
        }
        self.halt.send_replace(true);
        end
    }

    async fn open(&self, kind: ChannelKind, hello: Hello) -> Result<Channel, SessionEnd> {
        self.dialer.open(kind, hello).await.map_err(|e| match e {
            DialError::Lost(why) | DialError::Unreachable(why) => SessionEnd::Lost(why),
            DialError::Rejected => SessionEnd::Rejected,
        })
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
                Ok(Ok(None)) => {
                    return SessionEnd::Lost(
                        "the server closed the connection before it was ready".into(),
                    );
                }
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
        match self
            .outage
            .lock()
            .expect("outage lock")
            .recovered(Instant::now())
        {
            Some((down, attempts)) if attempts > 0 => info!(
                "connected to {} as \"{}\" (back after {}, {attempts} attempts)",
                s.server,
                s.name,
                human(down)
            ),
            _ => info!("connected to {} as \"{}\"", s.server, s.name),
        }

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
            let ports = svc.ports.max(1);
            let at = match svc.bind_port {
                0 => "private, for visitors".to_string(),
                p if ports > 1 => format!(
                    "server ports {}",
                    crate::config::check::ports::show(p, ports)
                ),
                p => format!("server port {p}"),
            };
            let target = match crate::config::check::parse_host_port(&svc.target) {
                Ok((_, p)) if ports > 1 => format!(
                    "{}:{}",
                    svc.target.rsplit_once(':').map_or("", |(h, _)| h),
                    crate::config::check::ports::show(p, ports)
                ),
                _ => svc.target.clone(),
            };
            info!(
                "service \"{}\": {} {at} -> {target}",
                svc.name,
                svc.protocol.as_str(),
            );
            data::precheck(&self.settings, svc);
        }
    }

    fn spawn_data(&self, token: [u8; TOKEN_LEN], opening: Arc<Semaphore>) {
        let me = Client {
            settings: self.settings.clone(),
            state: self.state.clone(),
            halt: self.halt.clone(),
            dialer: self.dialer.clone(),
            outage: self.outage.clone(),
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
        let started = Instant::now();
        let mut backoff = Backoff::new(Duration::from_millis(100), Duration::from_secs(1));
        loop {
            let permit = opening.clone().acquire_owned().await;
            let hello = Hello::Data {
                version: VERSION,
                token,
            };
            let ch = self.open(ChannelKind::Data, hello).await;
            drop(permit);
            match ch {
                Ok(ch) => {
                    data::serve(self.settings.clone(), self.dialer.resolver.clone(), ch).await;
                    return;
                }
                Err(_) if started.elapsed() < DATA_PATIENCE => {
                    tokio::time::sleep(backoff.next_delay()).await;
                }
                Err(SessionEnd::Lost(why)) => {
                    debug!("data channel: {why}");
                    return;
                }
                Err(_) => {
                    debug!("data channel rejected");
                    return;
                }
            }
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
