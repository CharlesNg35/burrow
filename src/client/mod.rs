mod data;
pub mod reconnect;
pub mod reload;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, mpsc, watch};
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
const MAX_WAITING: usize = 4096;

pub(crate) struct Waiting(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for Waiting {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
const DATA_PATIENCE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Connecting,
    Connected,
    Rejected,
}

#[derive(Clone)]
struct Current {
    settings: Arc<ClientSettings>,
    dialer: Arc<Dialer>,
}

#[derive(Clone)]
pub(crate) struct Live(Arc<watch::Sender<Current>>);

impl Live {
    pub(crate) fn settings(&self) -> Arc<ClientSettings> {
        self.0.borrow().settings.clone()
    }

    fn dialer(&self) -> Arc<Dialer> {
        self.0.borrow().dialer.clone()
    }
}

pub struct Client {
    live: Live,
    resolver: Arc<dyn Resolver>,
    state: watch::Sender<State>,
    halt: watch::Sender<bool>,
    outage: Arc<Mutex<Outage>>,
    reloads: Option<mpsc::Receiver<ClientSettings>>,
    instance: [u8; crate::protocol::TOKEN_LEN],
    welcomed: Arc<Mutex<Option<Instant>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    Stopped,
    GaveUp(Duration, String),
    Rejected,
}

pub enum SessionEnd {
    Lost(String),
    Rejected,
    Removed,
    Stop,
    Reconnect,
}

fn dialer_for(settings: &ClientSettings, resolver: &Arc<dyn Resolver>) -> Arc<Dialer> {
    let mut dialer = Dialer::client(settings);
    dialer.resolver = resolver.clone();
    Arc::new(dialer)
}

async fn next_reload(reloads: &mut Option<mpsc::Receiver<ClientSettings>>) -> ClientSettings {
    loop {
        match reloads {
            Some(rx) => match rx.recv().await {
                Some(next) => return next,
                None => *reloads = None,
            },
            None => std::future::pending::<()>().await,
        }
    }
}

impl Client {
    pub fn new(settings: ClientSettings) -> Self {
        let (state, _) = watch::channel(State::Connecting);
        let resolver = crate::transport::resolve::system();
        let current = Current {
            dialer: dialer_for(&settings, &resolver),
            settings: Arc::new(settings),
        };
        Client {
            live: Live(Arc::new(watch::Sender::new(current))),
            resolver,
            state,
            halt: watch::Sender::new(false),
            outage: Arc::new(Mutex::new(Outage::default())),
            reloads: None,
            instance: {
                let mut id = [0u8; crate::protocol::TOKEN_LEN];
                getrandom::fill(&mut id).expect("operating system random source is available");
                id
            },
            welcomed: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        let settings = self.live.settings();
        self.live.0.send_replace(Current {
            dialer: dialer_for(&settings, &resolver),
            settings,
        });
        self.resolver = resolver;
        self
    }

    pub fn with_reloads(mut self, reloads: mpsc::Receiver<ClientSettings>) -> Self {
        self.reloads = Some(reloads);
        self
    }

    fn apply(&self, next: ClientSettings, backoff: &mut Backoff) -> bool {
        let old = self.live.settings();
        let changes = reload::compare(&old, &next);
        info!("{}", changes.describe());
        if changes.is_empty() {
            return false;
        }
        if (old.reconnect_initial, old.reconnect_max)
            != (next.reconnect_initial, next.reconnect_max)
        {
            *backoff = Backoff::new(next.reconnect_initial, next.reconnect_max);
        }
        self.live.0.send_replace(Current {
            dialer: dialer_for(&next, &self.resolver),
            settings: Arc::new(next),
        });
        !changes.reconnect.is_empty()
    }

    pub fn state(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }

    pub async fn run(mut self, mut stop: watch::Receiver<bool>) -> RunEnd {
        let mut reloads = self.reloads.take();
        let first = self.live.settings();
        if first.insecure_plaintext {
            warn!("insecure_plaintext is enabled: forwarded TCP data is NOT encrypted");
        }
        let mut backoff = Backoff::new(first.reconnect_initial, first.reconnect_max);
        let mut rejected: Option<(Instant, u32)> = None;
        let end = loop {
            let end = {
                let session = self.session();
                tokio::pin!(session);
                loop {
                    tokio::select! {
                        e = &mut session => break e,
                        _ = stop.changed() => break SessionEnd::Stop,
                        next = next_reload(&mut reloads) => {
                            if self.apply(next, &mut backoff) {
                                break SessionEnd::Reconnect;
                            }
                        }
                    }
                }
            };
            let s = self.live.settings();
            let welcomed = self.welcomed.lock().expect("welcomed lock").take();
            if welcomed.is_some_and(|t| t.elapsed() > STABLE_AFTER) {
                backoff.reset();
            }
            let delay = backoff.next_delay();
            if !matches!(end, SessionEnd::Rejected) {
                rejected = None;
            }
            let why = match end {
                SessionEnd::Stop => break RunEnd::Stopped,
                SessionEnd::Reconnect => {
                    let _ = self.state.send(State::Connecting);
                    backoff.reset();
                    continue;
                }
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
                Report::First => match s.give_up_after {
                    Some(limit) => warn!(
                        "{why}; retrying in {:.1}s, for up to {} (reconnect.give_up_after_secs)",
                        delay.as_secs_f64(),
                        human(limit)
                    ),
                    None => warn!(
                        "{why}; retrying in {:.1}s, and will keep retrying",
                        delay.as_secs_f64()
                    ),
                },
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
                        "giving up: no working connection to {} for {} (reconnect.give_up_after_secs: {}); last error: {why}",
                        s.server,
                        human(down),
                        limit.as_secs()
                    );
                    break RunEnd::GaveUp(down, why);
                }
            }
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    _ = stop.changed() => return self.stopped(),
                    next = next_reload(&mut reloads) => {
                        if self.apply(next, &mut backoff) {
                            rejected = None;
                            let _ = self.state.send(State::Connecting);
                            backoff.reset();
                            break;
                        }
                    }
                }
            }
        };
        if end == RunEnd::Stopped {
            return self.stopped();
        }
        self.halt.send_replace(true);
        end
    }

    fn stopped(&self) -> RunEnd {
        info!("client stopped");
        self.halt.send_replace(true);
        RunEnd::Stopped
    }

    async fn open(&self, kind: ChannelKind, hello: Hello) -> Result<Channel, SessionEnd> {
        self.live
            .dialer()
            .open(kind, hello)
            .await
            .map_err(|e| match e {
                DialError::Lost(why) | DialError::Unreachable(why) => SessionEnd::Lost(why),
                DialError::Rejected => SessionEnd::Rejected,
            })
    }

    async fn session(&self) -> SessionEnd {
        let s = self.live.settings();
        let ch = match self
            .open(
                ChannelKind::Control,
                Hello::Control {
                    version: VERSION,
                    instance: self.instance,
                },
            )
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
        *self.welcomed.lock().expect("welcomed lock") = Some(Instant::now());
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

        let allow = ClientMsg::Allow(s.allow.describe());
        if writer.send(&mut wr, &allow.encode()).await.is_err() {
            return SessionEnd::Lost(format!("connection to {} lost", s.server));
        }
        let opening = Arc::new(Semaphore::new(MAX_OPENING));
        let waiting = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (ended, session_ended) = watch::channel(false);
        let _ended = CloseOnDrop(opening.clone(), ended);
        let strays = Arc::new(Strays::default());
        let timeout = heartbeat * 3;
        loop {
            let next = tokio::select! {
                r = tokio::time::timeout(timeout, reader.recv(&mut rd)) => r,
                _ = strays.seen.notified() => {
                    return SessionEnd::Lost(format!(
                        "{} does not know this session ({STRAYS} data channels were refused as stale), so its address may now lead to another server; reconnecting",
                        s.server
                    ));
                }
            };
            let msg = match next {
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
                    let now = waiting.load(std::sync::atomic::Ordering::Acquire);
                    let room = MAX_WAITING.saturating_sub(now);
                    for _ in 0..usize::from(count).min(room) {
                        waiting.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        let held = Waiting(waiting.clone());
                        self.spawn_data(
                            token,
                            opening.clone(),
                            session_ended.clone(),
                            held,
                            strays.clone(),
                        );
                    }
                    if usize::from(count) > room {
                        static MANY: crate::throttle::Throttle = crate::throttle::Throttle::new();
                        if let Some(more) = MANY.allow() {
                            warn!(
                                "the server asked for {count} more data channels while {now} are waiting; opening at most {MAX_WAITING}{more}"
                            );
                        }
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
            data::precheck(&self.live.settings(), svc);
        }
    }

    fn spawn_data(
        &self,
        token: [u8; TOKEN_LEN],
        opening: Arc<Semaphore>,
        ended: watch::Receiver<bool>,
        held: Waiting,
        strays: Arc<Strays>,
    ) {
        let me = Client {
            live: self.live.clone(),
            resolver: self.resolver.clone(),
            state: self.state.clone(),
            halt: self.halt.clone(),
            outage: self.outage.clone(),
            reloads: None,
            instance: self.instance,
            welcomed: self.welcomed.clone(),
        };
        let mut halted = self.halt.subscribe();
        tokio::spawn(async move {
            tokio::select! {
                _ = me.data_channel(token, opening, ended, held, strays) => {}
                _ = halted.wait_for(|h| *h) => {}
            }
        });
    }

    async fn data_channel(
        &self,
        token: [u8; TOKEN_LEN],
        opening: Arc<Semaphore>,
        ended: watch::Receiver<bool>,
        held: Waiting,
        strays: Arc<Strays>,
    ) {
        let started = Instant::now();
        let mut backoff = Backoff::new(Duration::from_millis(100), Duration::from_secs(1));
        loop {
            let Ok(permit) = opening.clone().acquire_owned().await else {
                return;
            };
            let hello = Hello::Data {
                version: VERSION,
                token,
            };
            let ch = self.live.dialer().open(ChannelKind::Data, hello).await;
            drop(permit);
            match ch {
                Ok(ch) => {
                    let stale =
                        data::serve(self.live.clone(), self.resolver.clone(), ch, ended, held)
                            .await;
                    if !stale {
                        strays.count.store(0, std::sync::atomic::Ordering::Relaxed);
                    } else if strays
                        .count
                        .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                        + 1
                        >= STRAYS
                    {
                        strays.seen.notify_one();
                    }
                    return;
                }
                Err(DialError::Rejected | DialError::Lost(_))
                    if started.elapsed() < DATA_PATIENCE =>
                {
                    tokio::time::sleep(backoff.next_delay()).await;
                }
                Err(_) if *ended.borrow() => return,
                Err(DialError::Lost(why) | DialError::Unreachable(why)) => {
                    static FAILED: crate::throttle::Throttle = crate::throttle::Throttle::new();
                    if let Some(more) = FAILED.allow() {
                        warn!("cannot open a data channel for the server: {why}{more}");
                    }
                    return;
                }
                Err(DialError::Rejected) => {
                    static REJECTED: crate::throttle::Throttle = crate::throttle::Throttle::new();
                    if let Some(more) = REJECTED.allow() {
                        warn!(
                            "the server refused a data channel; connections through this client wait or fail until it reconnects{more}"
                        );
                    }
                    return;
                }
            }
        }
    }
}

const STRAYS: usize = 3;

#[derive(Default)]
struct Strays {
    count: std::sync::atomic::AtomicUsize,
    seen: tokio::sync::Notify,
}

struct CloseOnDrop(Arc<Semaphore>, watch::Sender<bool>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
        self.1.send_replace(true);
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
        ByeReason::Duplicate => SessionEnd::Lost(
            "the server already has a client with this name connected from elsewhere; is a copy of this config running on another machine? this one keeps trying and takes over when the other leaves"
                .into(),
        ),
        ByeReason::PlaintextMismatch => SessionEnd::Lost(
            "server and client disagree on insecure_plaintext (set it the same on both sides)"
                .into(),
        ),
    }
}
